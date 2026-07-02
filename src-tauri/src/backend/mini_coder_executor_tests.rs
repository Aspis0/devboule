//! The mini-coder executor TEST battery — child module of
//! `mini_coder_executor.rs` (via `#[path]`), split out in the role-untangle
//! Phase 2 so the production file stays small. `super::*` still resolves to the
//! executor (and, via its wildcard re-exports, to the extracted
//! mini_edit_apply / mini_prompt / mini_command_build / agentic_worker items) —
//! this battery is the characterization guard for the pure move.

use super::mini_coder::MiniCoderResult;
use super::*;

fn directive(id: &str, parent: &str) -> MiniCoderDirective {
    MiniCoderDirective {
        id: id.into(),
        parent_agent_id: parent.into(),
        status: MiniCoderStatus::Pending,
        task: "docstring foo()".into(),
        files: vec!["src/a.rs".into()],
        backend: None,
        write: false,
        write_mode: mini_coder::WriteMode::EmitEdits,
        tier: Default::default(),
        project_id: None,
        allow_oracle: false,
        kill_requested: false,
        steer_queue: Vec::new(),
        result_path: format!("{id}.json"),
        agent_id: None,
        created_at: "2026-06-06T00:00:00Z".into(),
        claimed_at: None,
        scratch_path: None,
        started_at: None,
        result: None,
        attempt: 0,
        parent_directive_id: None,
        pigeon_ticket: None,
    }
}

#[test]
fn mini_agent_id_is_allowlist_safe_and_namespaced() {
    let d = directive("abcd1234ef", "coder-1717459200000");
    let id = mini_agent_id(&d);
    assert!(id.starts_with("mini-"));
    // Only [A-Za-z0-9._-] (matches agent_pty::validate_agent_id allowlist).
    assert!(
        crate::backend::agent_pty::validate_agent_id(&id).is_ok(),
        "id: {id}"
    );
    // Parent short is the alnum head (no '-'): "coder171".
    assert!(id.contains("coder171"), "id: {id}");
    assert!(id.contains("abcd1234"), "id: {id}");
}

#[test]
fn mini_agent_id_handles_empty_components() {
    let d = directive("", "");
    let id = mini_agent_id(&d);
    assert_eq!(id, "mini-p-x");
    assert!(crate::backend::agent_pty::validate_agent_id(&id).is_ok());
}

#[test]
fn parent_is_gone_detects_absent_and_closed() {
    use crate::backend::model::AgentLiveState;
    let mut state = AgentLiveState {
        version: 2,
        updated_at: String::new(),
        sessions: Vec::new(),
        claims: Vec::new(),
        events: Vec::new(),
        rules: Vec::new(),
        state_path: String::new(),
        mcp_command: String::new(),
        mcp_client_config: String::new(),
        mini_coder_directives: Vec::new(),
        visual_check_directives: Vec::new(),
        design_request_directives: Vec::new(),
        git_push_requests: Vec::new(),
        plan_approval_requests: Vec::new(),
        consent_requests: Vec::new(),
    };
    // Absent parent -> gone.
    assert!(parent_is_gone(&state, "coder-1"));
    // Active parent -> not gone.
    state.sessions.push(test_session("coder-1", "active"));
    assert!(!parent_is_gone(&state, "coder-1"));
    // Closed parent -> gone.
    state.sessions.push(test_session("coder-2", "closed"));
    assert!(parent_is_gone(&state, "coder-2"));
}

#[test]
fn result_rel_path_traversal_is_rejected_before_launch() {
    // WARNING 4: claim_and_launch validates directive.result_path with the SAME
    // gate the result reader uses; a `..`/absolute path must be rejected (the
    // claim fails) so the write/read target can never escape the scratch dir.
    assert!(mini_coder::validate_result_rel_path("../../etc/passwd").is_err());
    assert!(mini_coder::validate_result_rel_path("sub/../../escape.json").is_err());
    #[cfg(windows)]
    assert!(mini_coder::validate_result_rel_path("C:\\Windows\\x.json").is_err());
    #[cfg(not(windows))]
    assert!(mini_coder::validate_result_rel_path("/etc/passwd").is_err());
    // A normal relative path under the scratch dir is accepted.
    assert!(mini_coder::validate_result_rel_path("d1.json").is_ok());
    assert!(mini_coder::validate_result_rel_path("nested/d1.json").is_ok());
}

#[test]
fn read_result_outcome_missing_file_is_failed() {
    let dir = std::env::temp_dir().join(format!("mc_exec_missing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let outcome = read_result_outcome(&dir, "nope.json");
    assert_eq!(outcome.status, MiniCoderStatus::Failed);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn read_result_outcome_valid_done_after_canonicalize() {
    let dir = std::env::temp_dir().join(format!("mc_exec_done_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("d1.json"),
        r#"{"status":"done","output":"ok","filesTouched":["src/a.rs"]}"#,
    )
    .unwrap();
    let outcome = read_result_outcome(&dir, "d1.json");
    assert_eq!(
        outcome.status,
        MiniCoderStatus::Done,
        "err: {:?}",
        outcome.error
    );
    assert_eq!(outcome.output.as_deref(), Some("ok"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn result_json_with_hostile_output_round_trips_through_reader() {
    // A result whose output contains a double-quote, a backslash, and a newline
    // must serialize to VALID JSON (serde_json escaping) and read back to a clean
    // `done` outcome whose output is EXACT. Guards the result-file contract the
    // ollama/api stdout wrapper and the codex self-write both target.
    use super::super::mini_coder::MiniCoderResult;
    let output = "fixed \"foo\" in C:\\src\\a.rs\nran tests";
    let result = MiniCoderResult {
        status: "done".to_string(),
        output: Some(output.to_string()),
        ..Default::default()
    };
    let json = serde_json::to_string(&result).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(parsed["status"], "done");
    assert_eq!(parsed["output"], output);

    // And the executor's own read path resolves it to a `done` with the output.
    let dir = std::env::temp_dir().join(format!("mc_hostile_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("h.json"), &json).unwrap();
    let outcome = read_result_outcome(&dir, "h.json");
    assert_eq!(
        outcome.status,
        MiniCoderStatus::Done,
        "err: {:?}",
        outcome.error
    );
    assert_eq!(outcome.output.as_deref(), Some(output));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn finalize_reads_persisted_scratch_path_not_a_live_lookup() {
    // BLOCKER 3: the result is read from the scratch root PERSISTED on the
    // directive (`scratch_path`), so a parent that switched projects after launch
    // cannot redirect the read. We assert the invariant directly: the result lives
    // in dir A (the launch-time scratch on the directive); a DIFFERENT dir B (the
    // hypothetical post-switch project) does NOT contain it. `read_result_outcome`
    // keyed on the persisted dir A finds it; keyed on dir B fails.
    let a = std::env::temp_dir().join(format!("mc_scratch_a_{}", std::process::id()));
    let b = std::env::temp_dir().join(format!("mc_scratch_b_{}", std::process::id()));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("r.json"), r#"{"status":"done","output":"in A"}"#).unwrap();

    let mut d = directive("r", "coder-1");
    d.status = MiniCoderStatus::Running;
    d.result_path = "r.json".into();
    d.scratch_path = Some(a.to_string_lossy().to_string());

    // The persisted dir (A) is what finalize uses.
    let persisted = PathBuf::from(d.scratch_path.as_deref().unwrap());
    assert_eq!(persisted, a);
    let from_a = read_result_outcome(&persisted, &d.result_path);
    assert_eq!(from_a.status, MiniCoderStatus::Done);
    assert_eq!(from_a.output.as_deref(), Some("in A"));
    // A re-resolution to the switched project (B) would NOT find the result.
    let from_b = read_result_outcome(&b, &d.result_path);
    assert_eq!(from_b.status, MiniCoderStatus::Failed);

    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

// INTEGRATION (Windows): the REAL headless one-shot backend. Build the command
// the executor builds, spawn it through portable-pty exactly as `spawn_agent_pty`
// does, drive the master until the child writes its result file and EOFs, then
// assert the file holds a valid `done` result AND `read_result_outcome` resolves
// it to MiniCoderStatus::Done with the task echoed. Proves spawn -> one-shot ->
// result-file -> EOF -> read WITHOUT a full Tauri AppHandle (the loop's lock
// plumbing is covered by the pure unit tests above + the Python poll tests).
// Ignored by default; run locally with `cargo test -- --ignored`.
#[cfg(windows)]
#[test]
#[ignore = "spawns a real PTY child; run locally with --ignored"]
fn headless_one_shot_writes_result_and_eofs() {
    use portable_pty::PtySize;
    use std::io::{Read, Write};
    use std::time::Instant;

    let scratch = std::env::temp_dir().join(format!("mc_oneshot_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let result_target = scratch.join("d1.json");
    let project_root = std::env::temp_dir();
    // BLOCKER 2: a hostile task (double-quote + backslash + newline) must survive
    // the real PTY one-shot write and read back EXACTLY via serde_json escaping.
    let task = "docstring \"foo\" in C:\\src\\a.rs\nplease";

    let cmd = build_headless_mini_command(&project_root, &result_target, task).unwrap();

    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 32,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut child = pair.slave.spawn_command(cmd).expect("spawn");
    drop(pair.slave);

    // Answer ConPTY's startup DSR so the child's render pipeline does not stall.
    let mut writer = pair.master.take_writer().expect("writer");
    let _ = writer.write_all(b"\x1b[1;1R");
    let _ = writer.flush();

    // Drain the master on a reader thread until EOF (the one-shot exits).
    let mut reader = pair.master.try_clone_reader().expect("reader");
    let handle = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });

    // Poll for the result file to appear (the child writes it then exits).
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !result_target.exists() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(result_target.exists(), "mini must write its result file");

    // Close the master so the reader EOFs, then reap the child (no zombie).
    drop(pair.master);
    let join_deadline = Instant::now() + Duration::from_secs(5);
    while !handle.is_finished() && Instant::now() < join_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = handle.join();
    let _ = child.wait();

    // The executor's read path resolves it to a clean `done` outcome.
    let outcome = read_result_outcome(&scratch, "d1.json");
    assert_eq!(
        outcome.status,
        MiniCoderStatus::Done,
        "err: {:?}",
        outcome.error
    );
    assert_eq!(outcome.output.as_deref(), Some(task));
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn upsert_mini_session_stamps_parent_project_so_the_rail_groups_it() {
    // The mini must carry its coder's project, or ProjectsView.sessionsByProject
    // (keyed on current_project_id) filters it out and it never reaches the rail.
    let mut state = crate::backend::model::AgentLiveState {
        version: 2,
        updated_at: String::new(),
        sessions: Vec::new(),
        claims: Vec::new(),
        events: Vec::new(),
        rules: Vec::new(),
        state_path: String::new(),
        mcp_command: String::new(),
        mcp_client_config: String::new(),
        mini_coder_directives: Vec::new(),
        visual_check_directives: Vec::new(),
        design_request_directives: Vec::new(),
        git_push_requests: Vec::new(),
        plan_approval_requests: Vec::new(),
        consent_requests: Vec::new(),
    };
    upsert_mini_session(
        &mut state,
        "mini-c-1",
        "coder-1",
        Some("p1".into()),
        "2026-06-06T00:00:00Z",
        "ollama",
        None,
        crate::backend::mini_coder::DirectiveTier::Mini,
    );
    let mini = state
        .sessions
        .iter()
        .find(|s| s.agent_id == "mini-c-1")
        .expect("mini session inserted");
    assert_eq!(mini.current_project_id.as_deref(), Some("p1"));
    assert_eq!(mini.parent_agent_id.as_deref(), Some("coder-1"));
    assert_eq!(mini.client.as_deref(), Some("ollama"));
    assert_eq!(mini.host.as_deref(), Some(super::super::agents::HOST_APP));

    // A later re-upsert with a TRANSIENT None must NOT clear the resolved project
    // (a momentarily-empty parent snapshot mustn't drop the mini from the rail).
    upsert_mini_session(
        &mut state,
        "mini-c-1",
        "coder-1",
        None,
        "2026-06-06T00:01:00Z",
        "ollama",
        None,
        crate::backend::mini_coder::DirectiveTier::Mini,
    );
    let mini = state
        .sessions
        .iter()
        .find(|s| s.agent_id == "mini-c-1")
        .unwrap();
    assert_eq!(
        mini.current_project_id.as_deref(),
        Some("p1"),
        "transient None cleared the project"
    );
}

#[test]
fn upsert_mini_session_stores_mini_role_and_token_hash_when_granted() {
    // P3: a granted mini's session pins role "mini" + the launch-token HASH,
    // so MCP registration is token-bound and the stored role caps what the
    // mini may register as. An ungranted mini keeps the status-quo row.
    let mut state = empty_state();
    upsert_mini_session(
        &mut state,
        "mini-g-1",
        "coder-1",
        Some("p1".into()),
        "2026-06-12T00:00:00Z",
        "codex",
        Some("hash-0123456789abcdef0123456789abcdef"),
        crate::backend::mini_coder::DirectiveTier::Mini,
    );
    let mini = state
        .sessions
        .iter()
        .find(|s| s.agent_id == "mini-g-1")
        .expect("granted mini inserted");
    assert_eq!(mini.role, "mini");
    assert_eq!(
        mini.launch_token_hash.as_deref(),
        Some("hash-0123456789abcdef0123456789abcdef")
    );
    assert!(
        mini.launch_token_issued_at.is_some(),
        "issued_at must be stamped with the hash"
    );

    let mut state = empty_state();
    upsert_mini_session(
        &mut state,
        "mini-u-1",
        "coder-1",
        Some("p1".into()),
        "2026-06-12T00:00:00Z",
        "ollama",
        None,
        crate::backend::mini_coder::DirectiveTier::Mini,
    );
    let mini = state
        .sessions
        .iter()
        .find(|s| s.agent_id == "mini-u-1")
        .expect("ungranted mini inserted");
    assert_eq!(mini.role, "coder", "ungranted mini keeps the status quo");
    assert!(mini.launch_token_hash.is_none());
    assert!(mini.launch_token_issued_at.is_none());
}

fn p4_edit(path: &str, old: &str, new: &str) -> mini_coder::MiniEdit {
    mini_coder::MiniEdit {
        path: path.into(),
        old_string: old.into(),
        new_string: new.into(),
    }
}

fn p4_temp_project(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aspis-p4a-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp project dir");
    dir
}

#[test]
fn apply_edits_happy_path_in_order_with_ground_truth_and_preimage_hook() {
    let root = p4_temp_project("happy");
    std::fs::write(root.join("a.txt"), "alpha beta\n").unwrap();
    let allow = vec!["a.txt".to_string(), "new.txt".to_string()];
    let edits = vec![
        p4_edit("a.txt", "alpha", "ALPHA"),
        p4_edit("new.txt", "", "created\n"),
        p4_edit("a.txt", "beta", "BETA"),
    ];
    let mut pre: Vec<String> = Vec::new();
    let result = apply_emitted_edits(&root, &allow, &edits, |rel| pre.push(rel.to_string()))
        .expect("happy path applies");
    // Ground truth: first-touch order, deduped.
    assert_eq!(
        result.applied,
        vec!["a.txt".to_string(), "new.txt".to_string()]
    );
    // The pre-image hook fired once per touched file, in flush order.
    assert_eq!(pre, result.applied);
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "ALPHA BETA\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("new.txt")).unwrap(),
        "created\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn apply_edits_atomic_nothing_written_on_late_anchor_failure() {
    let root = p4_temp_project("atomic");
    std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
    let allow = vec!["a.txt".to_string()];
    let edits = vec![
        p4_edit("a.txt", "alpha", "ALPHA"),
        p4_edit("a.txt", "NO-SUCH-ANCHOR", "x"),
    ];
    let err = apply_emitted_edits(&root, &allow, &edits, |_| {}).unwrap_err();
    assert!(err.contains("matches 0 times"), "wrong error: {err}");
    // Pass-1 failed -> pass-2 never ran -> the file is byte-identical.
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "alpha\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn coverage_counts_only_fine_runners_rust_uncovered_python_covered() {
    // BLOCKER-2: "covered" must reflect what the PER-ROUND verdict (Fine pass) actually
    // exercises, NOT all applicable runners. RUST's language-specific runners (clippy/
    // cargo-check/cargo-audit/cargo-deny/cargo-fmt) are ALL Coarse, so a Rust file adds
    // ZERO Fine runners over the cross-cutting Fine baseline -> NOT covered (budget 1, no
    // per-round Rust feedback to iterate against). PYTHON's ruff/ruff-format/pyright/
    // bandit/vulture are Fine -> covered (budget N). Both projects carry the matching
    // manifest so `detect_project_kinds` recognizes the kind.
    let rust_root = p4_temp_project("cov-rust");
    std::fs::write(rust_root.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    assert!(
        !directive_has_tier_a_coverage(&rust_root, &["src/a.rs".to_string()]),
        "Rust is Coarse-only for its lang-specific runners -> must be UNCOVERED"
    );
    std::fs::remove_dir_all(&rust_root).ok();

    let py_root = p4_temp_project("cov-python");
    std::fs::write(py_root.join("pyproject.toml"), "[project]\nname=\"x\"\n").unwrap();
    assert!(
        directive_has_tier_a_coverage(&py_root, &["src/a.py".to_string()]),
        "Python has Fine lang-specific runners (ruff/pyright/bandit/...) -> must be COVERED"
    );
    // A Rust file inside a Python project is still uncovered: it has NO Python Fine
    // runner and Rust's own runners need the Rust kind (absent here) -> baseline only.
    assert!(
        !directive_has_tier_a_coverage(&py_root, &["src/a.rs".to_string()]),
        "a .rs file in a Python-only project gets only cross-cutting runners -> UNCOVERED"
    );
    // Mixed directive: ANY covered file flips the whole directive to covered.
    assert!(
        directive_has_tier_a_coverage(&py_root, &["src/a.rs".to_string(), "src/b.py".to_string()]),
        "a directive with >=1 covered (.py) file is COVERED even alongside an uncovered .rs"
    );
    std::fs::remove_dir_all(&py_root).ok();
}

#[test]
fn coverage_empty_files_is_uncovered() {
    // Defensive: an empty file list can never be covered (matches the early return).
    let root = p4_temp_project("cov-empty");
    std::fs::write(root.join("pyproject.toml"), "[project]\n").unwrap();
    assert!(!directive_has_tier_a_coverage(&root, &[]));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn covered_languages_python_project_includes_python_excludes_rust() {
    // A3 helper: for a Python project the covered-language list MUST include Python
    // (ruff/pyright/bandit/vulture are Fine) and MUST exclude Rust (clippy/cargo-* are
    // all Coarse — the SAME Fine-over-baseline rule B2 uses). The manifest-free
    // languages (HTML/Shell/YAML/SQL/Dockerfile/GitHub Actions/CSS) gate on FileLang
    // alone, so they're covered in EVERY project. Result is deterministic + sorted.
    let py_root = p4_temp_project("langs-python");
    std::fs::write(py_root.join("pyproject.toml"), "[project]\nname=\"x\"\n").unwrap();
    let langs = tier_a_covered_languages(&py_root);
    assert!(
        langs.contains(&"Python"),
        "Python must be covered: {langs:?}"
    );
    assert!(
        !langs.contains(&"Rust"),
        "Rust is Coarse-only -> never covered: {langs:?}"
    );
    // TS/Go/C++/Kotlin need their own manifest (absent here) -> NOT covered.
    assert!(
        !langs.contains(&"Go"),
        "Go needs go.mod (absent) -> uncovered: {langs:?}"
    );
    assert!(
        !langs.contains(&"Kotlin"),
        "Kotlin needs Gradle (absent) -> uncovered: {langs:?}"
    );
    // Manifest-free languages are always covered.
    for l in [
        "HTML",
        "Shell",
        "YAML",
        "SQL",
        "Dockerfile",
        "GitHub Actions",
        "CSS",
    ] {
        assert!(
            langs.contains(&l),
            "manifest-free {l} must be covered: {langs:?}"
        );
    }
    // Deterministic + sorted.
    let mut sorted = langs.clone();
    sorted.sort_unstable();
    assert_eq!(langs, sorted, "covered languages must be sorted: {langs:?}");
    assert_eq!(
        langs,
        tier_a_covered_languages(&py_root),
        "must be deterministic"
    );
    std::fs::remove_dir_all(&py_root).ok();
}

#[test]
fn covered_languages_rust_only_project_excludes_rust() {
    // A Rust-only project: Rust's language-specific runners are ALL Coarse, so Rust is
    // NOT in the covered list (agentic-iterative on .rs buys no per-round feedback).
    // Only the manifest-free baseline languages remain. No Python/TS/Go/etc. (no
    // matching manifest).
    let rust_root = p4_temp_project("langs-rust");
    std::fs::write(rust_root.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    let langs = tier_a_covered_languages(&rust_root);
    assert!(
        !langs.contains(&"Rust"),
        "Rust must NOT be covered (Coarse-only): {langs:?}"
    );
    assert!(
        !langs.contains(&"Python"),
        "no Python manifest -> uncovered: {langs:?}"
    );
    assert!(
        !langs.contains(&"TypeScript/JavaScript"),
        "no Node manifest -> uncovered: {langs:?}"
    );
    // The manifest-free baseline is still present (kind-gate-free).
    assert!(
        langs.contains(&"HTML") && langs.contains(&"Shell"),
        "baseline langs present: {langs:?}"
    );
    std::fs::remove_dir_all(&rust_root).ok();
}

#[test]
fn covered_languages_node_project_includes_ts() {
    // A Node project adds TypeScript/JavaScript (eslint/oxlint/prettier are Fine)
    // to the covered set; Rust still excluded.
    let node_root = p4_temp_project("langs-node");
    std::fs::write(node_root.join("package.json"), "{\"name\":\"x\"}\n").unwrap();
    let langs = tier_a_covered_languages(&node_root);
    assert!(
        langs.contains(&"TypeScript/JavaScript"),
        "TS must be covered: {langs:?}"
    );
    assert!(!langs.contains(&"Rust"), "Rust excluded: {langs:?}");
    std::fs::remove_dir_all(&node_root).ok();
}

#[test]
fn apply_edits_rejects_allowlist_miss_traversal_and_case_variant() {
    let root = p4_temp_project("allow");
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    let allow = vec!["main.rs".to_string()];
    for bad in ["other.rs", "../main.rs", "/etc/hosts", "Main.RS"] {
        let err =
            apply_emitted_edits(&root, &allow, &[p4_edit(bad, "fn", "FN")], |_| {}).unwrap_err();
        assert!(
            err.contains("edit 0"),
            "path {bad} must be rejected, got: {err}"
        );
    }
    // Untouched.
    assert_eq!(
        std::fs::read_to_string(root.join("main.rs")).unwrap(),
        "fn main() {}\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)]
#[test]
fn apply_edits_rejects_symlink_escape() {
    let root = p4_temp_project("symlink");
    let outside = std::env::temp_dir().join(format!("aspis-p4a-outside-{}", std::process::id()));
    std::fs::write(&outside, "outside\n").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();
    let err = apply_emitted_edits(
        &root,
        &["link.txt".to_string()],
        &[p4_edit("link.txt", "outside", "INSIDE")],
        |_| {},
    )
    .unwrap_err();
    assert!(
        err.contains("escapes the project root"),
        "wrong error: {err}"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside\n");
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_file(&outside).ok();
}

#[test]
fn apply_edits_create_rules_and_caps() {
    let root = p4_temp_project("create");
    std::fs::write(root.join("a.txt"), "x\n").unwrap();
    // Create over an existing file is rejected.
    let err = apply_emitted_edits(
        &root,
        &["a.txt".to_string()],
        &[p4_edit("a.txt", "", "clobber")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("already exists"), "wrong error: {err}");
    // Create inside a missing directory is rejected (no implicit mkdir).
    let err = apply_emitted_edits(
        &root,
        &["newdir/f.txt".to_string()],
        &[p4_edit("newdir/f.txt", "", "content")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("does not exist"), "wrong error: {err}");
    // Duplicate create in one batch is rejected.
    let err = apply_emitted_edits(
        &root,
        &["b.txt".to_string()],
        &[p4_edit("b.txt", "", "one"), p4_edit("b.txt", "", "two")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("duplicate create"), "wrong error: {err}");
    // Caps: empty edits is a no-op Ok; >40 edits and an oversized allowlist reject.
    assert_eq!(
        apply_emitted_edits(&root, &["a.txt".to_string()], &[], |_| {})
            .unwrap()
            .applied,
        Vec::<String>::new()
    );
    let many: Vec<_> = (0..41).map(|_| p4_edit("a.txt", "x", "y")).collect();
    let err = apply_emitted_edits(&root, &["a.txt".to_string()], &many, |_| {}).unwrap_err();
    assert!(err.contains("too many edits"), "wrong error: {err}");
    let wide: Vec<String> = (0..11).map(|i| format!("f{i}.txt")).collect();
    let err = apply_emitted_edits(&root, &wide, &[p4_edit("f0.txt", "", "c")], |_| {}).unwrap_err();
    assert!(err.contains("1..=10"), "wrong error: {err}");
    std::fs::remove_dir_all(&root).ok();
}

fn p4_write_directive(files: &[&str]) -> MiniCoderDirective {
    let mut d = p4_directive(false);
    d.write = true;
    d.files = files.iter().map(|s| s.to_string()).collect();
    d
}

fn p4_done_with_edits(edits: Vec<mini_coder::MiniEdit>) -> MiniCoderOutcome {
    MiniCoderOutcome::done(mini_coder::MiniCoderResult {
        status: "done".into(),
        output: Some("did it".into()),
        files_touched: vec!["lie.txt".into()],
        edits,
        question: None,
        partial: None,
        net_blocked: false,
        folder_write_blocked: None,
    })
}

#[test]
fn write_apply_ground_truths_files_touched_and_clears_edits() {
    let root = p4_temp_project("wapply");
    std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
    let d = p4_write_directive(&["a.txt"]);
    let outcome = p4_done_with_edits(vec![p4_edit("a.txt", "alpha", "ALPHA")]);
    let (out, _diffs) = apply_write_directive_edits(Some(&root), &d, outcome);
    assert_eq!(out.status, MiniCoderStatus::Done);
    // The mini CLAIMED lie.txt; ground truth is what was actually applied.
    assert_eq!(out.files_touched, vec!["a.txt".to_string()]);
    assert!(out.edits.is_empty(), "edit bodies must not persist");
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "ALPHA\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn write_apply_failure_converts_done_to_failed() {
    let root = p4_temp_project("wfail");
    std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
    let d = p4_write_directive(&["a.txt"]);
    let outcome = p4_done_with_edits(vec![p4_edit("a.txt", "missing-anchor", "x")]);
    let (out, _diffs) = apply_write_directive_edits(Some(&root), &d, outcome);
    assert_eq!(out.status, MiniCoderStatus::Failed);
    assert!(
        out.error
            .as_deref()
            .unwrap_or("")
            .contains("emitted edits rejected"),
        "error missing: {:?}",
        out.error
    );
    // Atomicity: the failed apply wrote nothing.
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "alpha\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn non_write_directive_drops_edits_without_touching_disk() {
    let root = p4_temp_project("wdrop");
    std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
    // p4_directive(false) has write=false and files [src/a.rs, src/b.rs].
    let d = p4_directive(false);
    let outcome = p4_done_with_edits(vec![p4_edit("a.txt", "alpha", "ALPHA")]);
    let (out, _diffs) = apply_write_directive_edits(Some(&root), &d, outcome);
    assert_eq!(out.status, MiniCoderStatus::Done);
    assert!(out.edits.is_empty(), "untrusted edits must be dropped");
    // The model's claim passes through untouched on the no-write path...
    assert_eq!(out.files_touched, vec!["lie.txt".to_string()]);
    // ...and the disk was never touched.
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "alpha\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn write_apply_without_root_fails_closed() {
    let d = p4_write_directive(&["a.txt"]);
    let outcome = p4_done_with_edits(vec![p4_edit("a.txt", "alpha", "ALPHA")]);
    let (out, _diffs) = apply_write_directive_edits(None, &d, outcome);
    assert_eq!(out.status, MiniCoderStatus::Failed);
    assert!(
        out.error
            .as_deref()
            .unwrap_or("")
            .contains("without a resolvable project root"),
        "error missing: {:?}",
        out.error
    );
}

// ── FIX 3: apply_write_directive_edits must not zero was_net_blocked ──────

/// When `apply_write_directive_edits` converts an outcome to `failed`
/// (e.g. no project root), the returned outcome has net_blocked=false.
/// The caller in `finalize_finished_mini` must capture `was_net_blocked`
/// BEFORE the call so the consent-request check reads the pre-apply value.
///
/// This test documents the hazard: input net_blocked=true → output
/// net_blocked=false after the no-root failure path.
#[test]
fn apply_write_directive_edits_no_root_drops_net_blocked() {
    let d = p4_write_directive(&["a.txt"]);
    // Construct a done outcome that has net_blocked=true.
    let mut outcome = p4_done_with_edits(vec![p4_edit("a.txt", "alpha", "ALPHA")]);
    outcome.net_blocked = true;

    // Pre-capture (what the fixed caller does).
    let was_net_blocked = outcome.net_blocked;

    // apply_write_directive_edits with no root → returns MiniCoderOutcome::failed
    // which has net_blocked=false.
    let (applied, _diffs) = apply_write_directive_edits(None, &d, outcome);
    assert_eq!(applied.status, MiniCoderStatus::Failed);
    // The applied outcome has LOST the net_blocked flag.
    assert!(
        !applied.net_blocked,
        "apply_write_directive_edits::failed path zeros net_blocked (expected hazard)"
    );
    // The caller must use was_net_blocked, not applied.net_blocked.
    assert!(
        was_net_blocked,
        "was_net_blocked pre-captured correctly before apply_write_directive_edits"
    );
}

#[test]
fn apply_edits_rejects_existing_file_outside_allowlist() {
    // Review F4: the older allowlist-miss tests used files that do not
    // exist, so the canonicalize guard masked the allowlist check. This
    // pins the allowlist itself: the target EXISTS but is not listed.
    let root = p4_temp_project("allowpin");
    std::fs::write(root.join("listed.txt"), "x\n").unwrap();
    std::fs::write(root.join("present.txt"), "y\n").unwrap();
    let err = apply_emitted_edits(
        &root,
        &["listed.txt".to_string()],
        &[p4_edit("present.txt", "y", "z")],
        |_| {},
    )
    .unwrap_err();
    assert!(
        err.contains("not in the directive allowlist"),
        "must fail ON THE ALLOWLIST, got: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("present.txt")).unwrap(),
        "y\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn apply_edits_cross_file_atomicity() {
    // Review F5: a pass-1 failure on the SECOND file must leave the FIRST
    // file (whose edit validated fine) untouched on disk.
    let root = p4_temp_project("crossatomic");
    std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(root.join("b.txt"), "beta\n").unwrap();
    let allow = vec!["a.txt".to_string(), "b.txt".to_string()];
    let edits = vec![
        p4_edit("a.txt", "alpha", "ALPHA"),
        p4_edit("b.txt", "NO-SUCH-ANCHOR", "x"),
    ];
    let err = apply_emitted_edits(&root, &allow, &edits, |_| {}).unwrap_err();
    assert!(err.contains("matches 0 times"), "wrong error: {err}");
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "alpha\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("b.txt")).unwrap(),
        "beta\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn apply_edits_normalizes_cosmetic_path_variants_on_both_sides() {
    // Review F1: "./src/a.rs" in the directive vs "src/a.rs" emitted (and
    // vice versa) must MATCH — both sides share the lexical normalizer.
    let root = p4_temp_project("norm");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "one\n").unwrap();
    // Dotted allowlist, clean emitted path.
    let result = apply_emitted_edits(
        &root,
        &["./src/a.rs".to_string()],
        &[p4_edit("src/a.rs", "one", "two")],
        |_| {},
    )
    .expect("dotted allowlist must match clean path");
    assert_eq!(result.applied, vec!["src/a.rs".to_string()]);
    // Clean allowlist, dotted+doubled emitted path.
    let result = apply_emitted_edits(
        &root,
        &["src/a.rs".to_string()],
        &[p4_edit("./src//a.rs", "two", "three")],
        |_| {},
    )
    .expect("dotted emitted path must match clean allowlist");
    assert_eq!(result.applied, vec!["src/a.rs".to_string()]);
    assert_eq!(
        std::fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "three\n"
    );
    // An empty path is rejected outright.
    let err = apply_emitted_edits(
        &root,
        &["src/a.rs".to_string()],
        &[p4_edit("", "three", "x")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("empty path"), "wrong error: {err}");
    std::fs::remove_dir_all(&root).ok();
}

// -- Aider-style tiered fuzzy-match fallback ----------------------------

#[test]
fn fuzzy_exact_single_match_still_applies_via_exact_tier() {
    // Regression: the verbatim, exactly-once case is unchanged and records `exact`.
    let root = p4_temp_project("fz-exact");
    std::fs::write(root.join("a.txt"), "let x = 1;\nlet y = 2;\n").unwrap();
    let result = apply_emitted_edits(
        &root,
        &["a.txt".to_string()],
        &[p4_edit("a.txt", "let x = 1;", "let x = 42;")],
        |_| {},
    )
    .expect("exact match applies");
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "let x = 42;\nlet y = 2;\n"
    );
    assert_eq!(
        result.match_tiers,
        vec![("a.txt".to_string(), "exact".to_string())]
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_exact_multi_match_still_errors_no_fuzzy_fallthrough() {
    // An AMBIGUOUS exact match must NOT fall through to fuzzy — it errors, and
    // the file is untouched (atomicity).
    let root = p4_temp_project("fz-ambig-exact");
    std::fs::write(root.join("a.txt"), "dup\ndup\n").unwrap();
    let err = apply_emitted_edits(
        &root,
        &["a.txt".to_string()],
        &[p4_edit("a.txt", "dup", "X")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("matches 2 times"), "wrong error: {err}");
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "dup\ndup\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_whitespace_only_difference_applies_splicing_original_bytes() {
    // The file uses a TAB indent + a trailing space; the emitted anchor uses
    // 4-space indent and no trailing space. Whitespace-normalization matches them,
    // and the splice replaces the ORIGINAL bytes (tab indent) with new_string.
    let root = p4_temp_project("fz-ws");
    std::fs::write(
        root.join("a.rs"),
        "fn f() {\n\tlet a = 1; \n\tlet b = 2;\n}\n",
    )
    .unwrap();
    // Emitted old uses spaces + collapsed whitespace, no trailing space.
    let old = "fn f() {\n    let a = 1;\n    let b = 2;\n}";
    let result = apply_emitted_edits(
        &root,
        &["a.rs".to_string()],
        &[p4_edit(
            "a.rs",
            old,
            "fn f() {\n    let a = 100;\n    let b = 2;\n}",
        )],
        |_| {},
    )
    .expect("whitespace-normalized match applies");
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "fn f() {\n    let a = 100;\n    let b = 2;\n}\n"
    );
    assert_eq!(
        result.match_tiers,
        vec![("a.rs".to_string(), "whitespace".to_string())]
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_whitespace_ambiguous_declines_then_errors() {
    // Two whitespace-identical spans => Tier 2 declines; Tier 3 also can't pick a
    // clear winner (two identical ratios) => ERROR, nothing written.
    let root = p4_temp_project("fz-ws-ambig");
    // Both anchor lines whitespace-normalize to "foo bar" but NEITHER matches the
    // emitted "foo  bar" (double space) verbatim — so Tier 1 finds 0, Tier 2 finds
    // 2 (ambiguous, declines), Tier 3 ties (no margin) => ERROR.
    std::fs::write(root.join("a.txt"), "  foo bar\nmid\n\tfoo   bar\n").unwrap();
    let err = apply_emitted_edits(
        &root,
        &["a.txt".to_string()],
        &[p4_edit("a.txt", "foo  bar", "BAZ")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("matches 0 times"), "wrong error: {err}");
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "  foo bar\nmid\n\tfoo   bar\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_near_match_above_threshold_applies_to_correct_span() {
    // A 5-line block that differs from the file by ONE token (>= 0.92 ratio) and
    // is uniquely located => Tier 3 splices that window's ORIGINAL bytes.
    let root = p4_temp_project("fz-near");
    let file = "alpha line one\nbeta line two\ngamma line three\ndelta line four\nepsilon line five\nzzz unrelated tail\n";
    std::fs::write(root.join("a.txt"), file).unwrap();
    // old differs only in "three" -> "thRee" (a near miss, not exact, not pure ws).
    let old = "alpha line one\nbeta line two\ngamma line thRee\ndelta line four\nepsilon line five";
    let new = "ALPHA\nBETA\nGAMMA\nDELTA\nEPSILON";
    let result = apply_emitted_edits(
        &root,
        &["a.txt".to_string()],
        &[p4_edit("a.txt", old, new)],
        |_| {},
    )
    .expect("near match applies");
    // The first five lines (the matched window's original bytes) are replaced; the
    // unrelated tail survives byte-for-byte.
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "ALPHA\nBETA\nGAMMA\nDELTA\nEPSILON\nzzz unrelated tail\n"
    );
    assert_eq!(result.match_tiers.len(), 1);
    assert_eq!(result.match_tiers[0].0, "a.txt");
    assert!(
        result.match_tiers[0].1.starts_with("fuzzy:"),
        "expected fuzzy tier, got {}",
        result.match_tiers[0].1
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_below_threshold_errors_nothing_written() {
    // A block that shares structure but is too different (< 0.92) must ERROR — no
    // guessing. Assert the file is byte-identical afterward (atomicity).
    let root = p4_temp_project("fz-below");
    let file = "the quick brown fox\njumps over the lazy dog\nand keeps on running\n";
    std::fs::write(root.join("a.txt"), file).unwrap();
    let old = "completely different content here\nnothing alike at all whatsoever\nzero overlap with the file";
    let err = apply_emitted_edits(
        &root,
        &["a.txt".to_string()],
        &[p4_edit("a.txt", old, "X")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("matches 0 times"), "wrong error: {err}");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), file);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_ambiguous_two_near_windows_errors_no_corruption() {
    // TWO windows both clear 0.92 with near-identical ratios => the margin guard
    // fails => ERROR. A wrong pick here would silently corrupt the file.
    let root = p4_temp_project("fz-ambig");
    // Two blocks that each differ from `old` by the SAME single character, so their
    // ratios tie within the 0.05 margin.
    let file =
        "header\nrole alpha config\nvalue one\nseparator\nrole alpha config\nvalue one\nfooter\n";
    std::fs::write(root.join("a.txt"), file).unwrap();
    let old = "role alphX config\nvalue one";
    let err = apply_emitted_edits(
        &root,
        &["a.txt".to_string()],
        &[p4_edit("a.txt", old, "X")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("matches 0 times"), "wrong error: {err}");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), file);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_create_branch_unchanged() {
    // Empty old_string is still a CREATE, never a fuzzy match — and records no tier.
    let root = p4_temp_project("fz-create");
    let result = apply_emitted_edits(
        &root,
        &["new.txt".to_string()],
        &[p4_edit("new.txt", "", "fresh content\n")],
        |_| {},
    )
    .expect("create applies");
    assert_eq!(
        std::fs::read_to_string(root.join("new.txt")).unwrap(),
        "fresh content\n"
    );
    assert_eq!(result.applied, vec!["new.txt".to_string()]);
    // CREATE has no anchor => no match-tier entry.
    assert!(result.match_tiers.is_empty(), "CREATE must record no tier");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_batch_atomicity_one_unmatchable_edit_writes_nothing() {
    // A multi-edit batch where the first edit fuzzy-matches but a LATER edit has no
    // confident match => PASS 1 fails => PASS 2 never runs => BOTH files untouched.
    let root = p4_temp_project("fz-batch");
    let a = "fn alpha() {\n    return 1;\n}\n";
    let b = "totally unrelated baseline\n";
    std::fs::write(root.join("a.rs"), a).unwrap();
    std::fs::write(root.join("b.txt"), b).unwrap();
    let allow = vec!["a.rs".to_string(), "b.txt".to_string()];
    let edits = vec![
        // Whitespace near-miss on a.rs (would apply on its own).
        p4_edit(
            "a.rs",
            "fn alpha() {\n  return 1;\n}",
            "fn alpha() {\n    return 2;\n}",
        ),
        // No confident match anywhere in b.txt => kills the whole batch.
        p4_edit("b.txt", "nothing like this content at all here", "X"),
    ];
    let err = apply_emitted_edits(&root, &allow, &edits, |_| {}).unwrap_err();
    assert!(err.contains("matches 0 times"), "wrong error: {err}");
    // Both files byte-identical: the atomic guarantee held.
    assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), a);
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), b);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_helpers_locate_span_tiers_and_normalization() {
    // Unit-level: the cascade picks the right tier and span, and normalization
    // preserves line structure (a 1-line block never normalize-equals 2 lines).
    // Exact tier.
    let (span, tier) = locate_edit_span("aXbYc", "bY").expect("exact");
    assert_eq!(tier, MatchTier::Exact);
    assert_eq!(&"aXbYc"[span], "bY");
    // Whitespace tier: original has a tab + trailing spaces. `old` has no trailing
    // newline, so the span is the matched line's CONTENT (its own `\n` is left in
    // place — exactly what an exact newline-free match would do).
    let text = "head\n\tfoo  bar   \ntail\n";
    let (span, tier) = locate_edit_span(text, "foo bar").expect("ws");
    assert_eq!(tier, MatchTier::Whitespace);
    assert_eq!(&text[span], "\tfoo  bar   ");
    // Normalization preserves line boundaries.
    assert_eq!(normalize_ws_block("a\t b   c"), "a b c");
    assert_eq!(normalize_ws_block("  a \n  b  "), "a\nb");
    assert_ne!(normalize_ws_block("a b"), normalize_ws_block("a\nb"));
    // Ambiguous exact => Err (no fallthrough).
    assert!(locate_edit_span("xx", "x").is_err());
    // line_start_offsets: split('\n') semantics — a trailing `\n` yields a final
    // empty line (the duplicated sentinel), no trailing `\n` does not.
    assert_eq!(line_start_offsets("a\nbc\n"), vec![0, 2, 5, 5]);
    assert_eq!(line_start_offsets("abc"), vec![0, 3]);
}

#[test]
fn fuzzy_overlapping_windows_do_not_defeat_margin_single_match_applies() {
    // Regression for the overlap bug: a SINGLE genuine fuzzy region where the base
    // (3-line) window scores ~0.987 and the OVERLAPPING base+1 (4-line) window over
    // the SAME start scores ~0.974 — within the 0.05 margin. Before the fix, that
    // adjacent-size window of the SAME region was counted as the "second-best" and the
    // margin guard wrongly REFUSED. Now overlapping windows are excluded from the
    // margin, so the unique location applies. (Ratios pre-measured against similar
    // 2.7; the base-1 2-line window scores ~0.78, well below threshold.)
    let root = p4_temp_project("fz-overlap");
    let file =
        "aaaaaaaaaaaaaaaaaaaa one\nbbbbbbbbbbbbbbbbbbbb two\ncccccccccccccccccccc three\nx\n";
    std::fs::write(root.join("a.txt"), file).unwrap();
    // Near-miss: line 3 "three" -> "thRee" (one char), no trailing newline on `old`.
    let old = "aaaaaaaaaaaaaaaaaaaa one\nbbbbbbbbbbbbbbbbbbbb two\ncccccccccccccccccccc thRee";
    let new = "REPLACED";
    let result = apply_emitted_edits(
        &root,
        &["a.txt".to_string()],
        &[p4_edit("a.txt", old, new)],
        |_| {},
    )
    .expect("single overlapping-window match must APPLY (margin not defeated by overlap)");
    // `old` has no trailing newline, so the matched span is the three lines' CONTENT;
    // line 2's own `\n` is left in place, then the unrelated "x" tail survives.
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "REPLACED\nx\n"
    );
    assert_eq!(result.match_tiers.len(), 1);
    assert_eq!(result.match_tiers[0].0, "a.txt");
    assert!(
        result.match_tiers[0].1.starts_with("fuzzy:"),
        "expected fuzzy tier, got {}",
        result.match_tiers[0].1
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_whitespace_only_anchor_errors_no_eof_insertion() {
    // Regression for the silent-corruption bug: an all-whitespace `old_string` with no
    // exact match must ERROR, not insert at EOF. The file ends in `\n`, so its phantom
    // trailing empty line whitespace-normalizes to "" — exactly what the whitespace
    // anchor normalizes to. Without the guard, Tier 2 returned an EMPTY span at EOF and
    // the splice INSERTED `new_string` at end-of-file, leaving the real target intact.
    let root = p4_temp_project("fz-ws-only");
    let file = "fn main() {\n    let x = 1;\n}\n";
    std::fs::write(root.join("a.rs"), file).unwrap();
    // Whitespace-only anchor (spaces + tab), NOT present verbatim as a unique line.
    let err = apply_emitted_edits(
        &root,
        &["a.rs".to_string()],
        &[p4_edit("a.rs", "   \t  ", "INJECTED\n")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("matches 0 times"), "wrong error: {err}");
    // File is byte-for-byte unchanged: explicitly NO EOF insertion of "INJECTED".
    let after = std::fs::read_to_string(root.join("a.rs")).unwrap();
    assert_eq!(after, file);
    assert!(
        !after.contains("INJECTED"),
        "EOF insertion leaked: {after:?}"
    );
    // Direct unit check: the whitespace tier itself declines an all-whitespace anchor.
    assert!(find_whitespace_span(file, "   \t  ").is_none());
    assert!(find_whitespace_span(file, " ").is_none());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn fuzzy_skipped_on_large_file_errors_instead_of_scanning() {
    // DoS guard: a file larger than FUZZY_MAX_FILE_BYTES skips Tier 3 entirely. A
    // near-but-not-exact `old_string` (no exact/whitespace anchor) therefore ERRORS
    // immediately rather than running the O(windows x Myers) scan. Exact/whitespace
    // tiers still run, so an exact anchor on a large file would still match (not tested
    // here — the point is that the *fuzzy* tier is bypassed).
    let root = p4_temp_project("fz-large");
    // Build a > 256 KiB file of distinct lines so nothing matches verbatim.
    let mut file = String::with_capacity(FUZZY_MAX_FILE_BYTES + 4096);
    let mut n = 0usize;
    while file.len() <= FUZZY_MAX_FILE_BYTES {
        file.push_str(&format!("line number {n} with some filler text here\n"));
        n += 1;
    }
    assert!(
        file.len() > FUZZY_MAX_FILE_BYTES,
        "test file must exceed the cap"
    );
    std::fs::write(root.join("big.txt"), &file).unwrap();
    // A near-miss against one real line (one char changed) — would fuzzy-match on a
    // small file, but the size cap bypasses Tier 3 so it errors.
    let old = "line number 3 with sNme filler text here";
    // Sanity: the fuzzy helper itself returns None purely from the size cap.
    assert!(find_fuzzy_span(&file, old).is_none());
    let err = apply_emitted_edits(
        &root,
        &["big.txt".to_string()],
        &[p4_edit("big.txt", old, "X")],
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("matches 0 times"), "wrong error: {err}");
    assert_eq!(std::fs::read_to_string(root.join("big.txt")).unwrap(), file);
    std::fs::remove_dir_all(&root).ok();
}

fn empty_state() -> crate::backend::model::AgentLiveState {
    crate::backend::model::AgentLiveState {
        version: 2,
        updated_at: String::new(),
        sessions: Vec::new(),
        claims: Vec::new(),
        events: Vec::new(),
        rules: Vec::new(),
        state_path: String::new(),
        mcp_command: String::new(),
        mcp_client_config: String::new(),
        mini_coder_directives: Vec::new(),
        visual_check_directives: Vec::new(),
        design_request_directives: Vec::new(),
        git_push_requests: Vec::new(),
        plan_approval_requests: Vec::new(),
        consent_requests: Vec::new(),
    }
}

#[test]
fn snapshot_parent_project_reads_purely_from_the_given_snapshot() {
    // BLOCKER 1: the parent's project is resolved from the SAME pass snapshot
    // plan_tick saw — a later mutation of the live state must NOT change the value
    // the mini is launched with. We model that by resolving twice: once against
    // the captured snapshot (the pass snapshot), once against a mutated one. The
    // captured one wins (it is the value passed into claim_and_launch).
    let mut snapshot = empty_state();
    let mut parent = test_session("coder-1", "active");
    parent.current_project_id = Some("p1".into());
    snapshot.sessions.push(parent);

    // The value the executor pins for this pass.
    assert_eq!(
        snapshot_parent_project(&snapshot, "coder-1").as_deref(),
        Some("p1")
    );

    // A LATER mutation (parent switches project / goes None) is on a DIFFERENT
    // snapshot and cannot retroactively change the pinned value above.
    let mut later = snapshot.clone();
    later.sessions[0].current_project_id = Some("p2".into());
    assert_eq!(
        snapshot_parent_project(&later, "coder-1").as_deref(),
        Some("p2")
    );
    // The pass snapshot is unchanged — the mini still launches into p1.
    assert_eq!(
        snapshot_parent_project(&snapshot, "coder-1").as_deref(),
        Some("p1")
    );
}

/// WARNING 3 (REDUNDANT SNAPSHOTS + TOCTOU): project_id AND trusted are derived from
/// ONE snapshot. The pure resolver maps the directive's agent_id -> session ->
/// current_project_id, then feeds THAT id to the trust lookup — the two can never
/// diverge (findings for p1 / trust for p2 was the bug).
#[test]
fn resolve_project_and_trust_derives_both_from_one_snapshot() {
    let mut snapshot = empty_state();
    let mut sess = test_session("mini-c-d1", "active");
    sess.current_project_id = Some("p1".into());
    snapshot.sessions.push(sess);
    let mut d = directive("d1", "coder-1");
    d.agent_id = Some("mini-c-d1".into());

    // The trust lookup is called with EXACTLY the project resolved from the snapshot.
    let mut seen: Option<String> = None;
    let (pid, trusted) = resolve_project_and_trust(Some(&snapshot), &d, |p| {
        seen = Some(p.to_string());
        p == "p1" // trusted for p1
    });
    assert_eq!(pid.as_deref(), Some("p1"));
    assert!(trusted, "p1 is trusted");
    assert_eq!(
        seen.as_deref(),
        Some("p1"),
        "trust checked for the SAME project"
    );
}

/// WARNING 3: a missing snapshot / agent_id / session yields (None, false) and never
/// invokes the trust lookup — fail-closed (never lint an unresolvable tree).
#[test]
fn resolve_project_and_trust_fails_closed_when_unresolvable() {
    let d = directive("d1", "coder-1"); // agent_id = None
    let mut called = false;
    let (pid, trusted) = resolve_project_and_trust(None, &d, |_p| {
        called = true;
        true
    });
    assert_eq!(pid, None);
    assert!(!trusted);
    assert!(
        !called,
        "trust lookup never runs for an unresolvable project"
    );
}

/// BLOCKER 2 (TIMEOUT EXCLUSION): a directive whose deferred-verdict thread is in
/// flight is Running with a long-elapsed started_at but must NOT be timed out.

/// BLOCKER 2 (IN-FLIGHT GUARD): only ONE verdict thread per directive can be claimed;
/// a second claim for the same id fails until released. This is what stops `run_pass`

/// BLOCKER 1: a POISONED `verdict_inflight` mutex must NOT silently disable the
/// timeout-exclusion (return empty) nor no-op a release. After poisoning the lock from
/// a panicking thread, `verdict_inflight_ids` still returns the LIVE set and

/// BLOCKER 2 (RAII): the in-flight id is released on EVERY exit path of the verdict
/// thread body — even when BOTH the work closure AND the fail-closed closure panic.
/// The `VerdictInflightGuard`'s `Drop` runs during unwind, so the id never leaks (a

/// BLOCKER 2 (RAII): the guard also releases on the NORMAL (no-panic) path and on a

/// WARNING 6: the verdict thread threads the executor's REAL running/stop flag (so an
/// in-flight linter run honors app exit) — NOT a throwaway `AtomicBool(true)`. The
/// plumbing: `running_flag()` hands out a CLONE of the same `Arc<AtomicBool>` the loop

/// WARNING 3 (self-healing): the SAME reconcile logic the startup sweep uses is the
/// one `run_pass` folds into every steady-state tick — so an Failed whose retry
/// child is TERMINAL is flagged for reconcile from the pass directives, not only at
/// startup. (`reconcile_awaiting_retry_orphans` + `run_pass` apply this against the
/// live state under the lock; here we assert the decision the steady-state pass acts

#[test]
fn snapshot_parent_project_is_none_when_parent_absent_or_projectless() {
    // Parent absent -> None (claim_and_launch fails the directive cleanly).
    let snapshot = empty_state();
    assert_eq!(snapshot_parent_project(&snapshot, "coder-1"), None);

    // Parent present but carrying no project -> None as well.
    let mut snapshot = empty_state();
    snapshot.sessions.push(test_session("coder-1", "active")); // current_project_id = None
    assert_eq!(snapshot_parent_project(&snapshot, "coder-1"), None);
}

#[test]
fn close_mini_session_marks_done_so_the_rail_excludes_it() {
    // WARNING 3: after a mini directive reaches a terminal outcome, its SESSION is
    // closed (status "done") so isRecentProjectSession (TS) drops it from the rail
    // instead of letting it linger ~15min as a stale active agent.
    let mut state = empty_state();
    upsert_mini_session(
        &mut state,
        "mini-c-1",
        "coder-1",
        Some("p1".into()),
        "2026-06-06T00:00:00Z",
        "ollama",
        None,
        crate::backend::mini_coder::DirectiveTier::Mini,
    );
    assert_eq!(
        state
            .sessions
            .iter()
            .find(|s| s.agent_id == "mini-c-1")
            .unwrap()
            .status,
        "active"
    );
    close_mini_session(&mut state, "mini-c-1");
    assert_eq!(
        state
            .sessions
            .iter()
            .find(|s| s.agent_id == "mini-c-1")
            .unwrap()
            .status,
        "done"
    );
    // A missing session is a no-op (no panic).
    close_mini_session(&mut state, "nope");
}

// -- P5: killRequested WINS + mini_coder_kill order-of-operations ---------

fn running_directive_with_scratch(id: &str, scratch: &std::path::Path) -> MiniCoderDirective {
    let mut d = directive(id, "coder-1");
    d.status = MiniCoderStatus::Running;
    d.agent_id = Some(format!("mini-c-{id}"));
    d.result_path = format!("{id}.json");
    d.scratch_path = Some(scratch.to_string_lossy().to_string());
    d
}

#[test]
fn finalize_outcome_kill_requested_wins_over_present_done_file() {
    // P5 RACE: the mini wrote a valid `done` result file in the SAME instant the
    // human hit Stop. killRequested WINS — the outcome is aborted_by_human and the
    // result file is NOT even read (a racing done must never overwrite the abort).
    let dir = std::env::temp_dir().join(format!("mc_p5_killwin_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("k1.json"),
        r#"{"status":"done","output":"raced done"}"#,
    )
    .unwrap();

    let mut d = running_directive_with_scratch("k1", &dir);
    d.kill_requested = true;
    let outcome = finalize_outcome(&d);
    assert_eq!(
        outcome.status,
        MiniCoderStatus::AbortedByHuman,
        "human Stop must win the same-instant done; err: {:?}",
        outcome.error
    );
    // The mini's racing output must NOT leak into the abort outcome.
    assert_ne!(outcome.output.as_deref(), Some("raced done"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn finalize_outcome_kill_requested_no_file_is_aborted_not_failed() {
    // killRequested + NO result file -> aborted_by_human (NOT failed). The human
    // asserted control; absence of a result is not a mini failure here.
    let dir = std::env::temp_dir().join(format!("mc_p5_killnofile_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = running_directive_with_scratch("k2", &dir);
    d.kill_requested = true;
    let outcome = finalize_outcome(&d);
    assert_eq!(outcome.status, MiniCoderStatus::AbortedByHuman);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn finalize_outcome_no_kill_no_file_is_failed_unchanged() {
    // killRequested=false + no result file -> failed (the pre-P5 behavior, intact).
    let dir = std::env::temp_dir().join(format!("mc_p5_nokill_nofile_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = running_directive_with_scratch("k3", &dir); // kill_requested = false
    let outcome = finalize_outcome(&d);
    assert_eq!(outcome.status, MiniCoderStatus::Failed);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn finalize_outcome_no_kill_with_done_file_is_done_unchanged() {
    // killRequested=false + a valid done file -> done (regression: the normal path).
    let dir = std::env::temp_dir().join(format!("mc_p5_nokill_done_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("k4.json"), r#"{"status":"done","output":"ok"}"#).unwrap();
    let d = running_directive_with_scratch("k4", &dir);
    let outcome = finalize_outcome(&d);
    assert_eq!(outcome.status, MiniCoderStatus::Done);
    assert_eq!(outcome.output.as_deref(), Some("ok"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn kill_requested_beats_timeout_in_transition_choice() {
    // The timeout path's locked closure consults the LIVE d.kill_requested: a
    // directive that BOTH blew its cap AND was Stopped reports aborted_by_human.
    let mut killed = directive("t1", "coder-1");
    killed.status = MiniCoderStatus::Running;
    killed.kill_requested = true;
    let aborted = if killed.kill_requested {
        mini_coder::apply_aborted(&killed, "stop").unwrap()
    } else {
        mini_coder::apply_timeout(&killed, "cap").unwrap()
    };
    assert_eq!(aborted.status, MiniCoderStatus::AbortedByHuman);

    // Without the kill flag the same code path yields timeout (unchanged).
    let mut not_killed = directive("t2", "coder-1");
    not_killed.status = MiniCoderStatus::Running;
    let timed = if not_killed.kill_requested {
        mini_coder::apply_aborted(&not_killed, "stop").unwrap()
    } else {
        mini_coder::apply_timeout(&not_killed, "cap").unwrap()
    };
    assert_eq!(timed.status, MiniCoderStatus::Timeout);
}

#[test]
fn kill_requested_beats_parent_gone_in_transition_choice() {
    // The parent-gone path's closure consults the LIVE d.kill_requested too: a
    // human Stop overrides a parent-gone verdict (aborted, not failed).
    let mut killed = directive("pg1", "coder-1");
    killed.status = MiniCoderStatus::Running;
    killed.kill_requested = true;
    let aborted = if killed.kill_requested {
        mini_coder::apply_aborted(&killed, "stop").unwrap()
    } else {
        mini_coder::apply_failed(&killed, "parent gone").unwrap()
    };
    assert_eq!(aborted.status, MiniCoderStatus::AbortedByHuman);
}

#[test]
fn mark_kill_requested_sets_flag_by_agent_id_before_any_kill() {
    // mini_coder_kill RECORDS killRequested (persisted) BEFORE the PTY kill. We
    // assert the recording step here: the flag is set on the directive whose
    // agentId matches, found, and idempotent on a re-mark.
    let mut state = empty_state();
    let mut d = directive("d1", "coder-1");
    d.status = MiniCoderStatus::Running;
    d.agent_id = Some("mini-c-d1".into());
    assert!(!d.kill_requested);
    state.mini_coder_directives.push(d);

    // Found + flagged. WARNING 6: returns the LIVE attempt's PTY id (here the matched
    // directive is itself the live attempt).
    assert_eq!(
        mark_kill_requested(&mut state, "mini-c-d1").as_deref(),
        Some("mini-c-d1")
    );
    assert!(state.mini_coder_directives[0].kill_requested);
    // Idempotent re-mark.
    assert_eq!(
        mark_kill_requested(&mut state, "mini-c-d1").as_deref(),
        Some("mini-c-d1")
    );
    assert!(state.mini_coder_directives[0].kill_requested);

    // WARNING 3: an unknown agentId (NOT a mini) is reported as None — the caller
    // must NOT kill any PTY for it (never kill a non-mini PTY).
    assert!(mark_kill_requested(&mut state, "mini-c-nope").is_none());
}

#[test]
fn mini_coder_kill_has_no_vault_unlock_gate() {
    // FIX 2 (SAFETY OVERRIDE): Stop must work even when the vault is LOCKED, so
    // `mini_coder_kill` must NOT depend on BackendState/`ensure_unlocked`. We assert
    // the gate is structurally absent by reading this module's own source: the kill
    // command's body must not call `ensure_unlocked`, and the only retained gate is
    // the mini-only `mark_kill_requested`. (A behavioral call needs an AppHandle;
    // this guards against a future re-introduction of the lock gate.)
    let src = include_str!("mini_coder_executor.rs");
    let fn_start = src
        .find("pub fn mini_coder_kill(")
        .expect("mini_coder_kill defined");
    let fn_end = src[fn_start..]
        .find("\n}\n")
        .map(|i| fn_start + i)
        .expect("function body end");
    let body = &src[fn_start..fn_end];
    // Look for the ACTUAL gate (a `.ensure_unlocked()` call) and the BackendState
    // parameter — not the bare word, which legitimately appears in the doc/comment
    // explaining WHY the gate was removed.
    assert!(
        !body.contains(".ensure_unlocked()"),
        "mini_coder_kill must NOT call ensure_unlocked (safety override): {body}"
    );
    assert!(
        !body.contains("BackendState"),
        "mini_coder_kill must NOT take the vault BackendState (safety override): {body}"
    );
    assert!(
        body.contains("mark_kill_requested"),
        "mini_coder_kill must keep the mini-only kill gate: {body}"
    );
    assert!(
        body.contains("validate_agent_id"),
        "mini_coder_kill must keep agent-id validation: {body}"
    );
}

#[test]
fn mark_kill_requested_skips_already_terminal_directive() {
    // WARNING 4: a mini that already reached a TERMINAL state must NOT get
    // killRequested set (terminal is terminal), and the fn reports not-found so the
    // caller does not kill a (non-existent) live PTY.
    let mut state = empty_state();
    let mut d = directive("d1", "coder-1");
    d.status = MiniCoderStatus::Done; // terminal
    d.agent_id = Some("mini-c-d1".into());
    state.mini_coder_directives.push(d);

    assert!(mark_kill_requested(&mut state, "mini-c-d1").is_none());
    assert!(
        !state.mini_coder_directives[0].kill_requested,
        "a terminal directive must NOT be flagged killRequested"
    );

    // A still-RUNNING mini IS flagged + reports its live PTY (contrast).
    let mut running = directive("d2", "coder-1");
    running.status = MiniCoderStatus::Running;
    running.agent_id = Some("mini-c-d2".into());
    state.mini_coder_directives.push(running);
    assert_eq!(
        mark_kill_requested(&mut state, "mini-c-d2").as_deref(),
        Some("mini-c-d2")
    );
    assert!(state.mini_coder_directives[1].kill_requested);
}

// -- P6: propagation + kill-chain + retry-lost ---------------------------

/// Build a chain: root (Failed) -> r1 (Failed) -> r2 (Running leaf).
/// Returns the state with the three directives.
fn three_deep_chain() -> crate::backend::model::AgentLiveState {
    let mut state = empty_state();
    let mut root = directive("root", "coder-1");
    root.status = MiniCoderStatus::Failed;
    let mut r1 = directive("root-r1", "coder-1");
    r1.status = MiniCoderStatus::Failed;
    r1.attempt = 1;
    r1.parent_directive_id = Some("root".into());
    let mut r2 = directive("root-r2", "coder-1");
    r2.status = MiniCoderStatus::Running;
    r2.attempt = 2;
    r2.parent_directive_id = Some("root".into());
    r2.agent_id = Some("mini-c-root-r2".into());
    state.mini_coder_directives.push(root);
    state.mini_coder_directives.push(r1);
    state.mini_coder_directives.push(r2);
    state
}

#[test]

/// WARNING 6 (KILL STALE AGENT_ID): stopping via an Failed PREDECESSOR's stale
/// agent id must return the LIVE retry's PTY (different agent id) — not the dead
/// predecessor's — so `mini_coder_kill` kills the attempt that actually has a PTY.
#[test]
#[test]

/// BLOCKER 1 (STRANDED ROOT): a retry that fails at LAUNCH must propagate `failed`
/// to its Failed root via the shared `stamp_terminal_and_propagate` (the exact
/// body `fail_launching` now runs under the lock). Before the fix `fail_launching`

/// BLOCKER 1 (second sweep rule): the sweep's body must catch an Failed
/// predecessor whose retry child is now TERMINAL (not absent) and re-propagate the
/// CHILD's terminal outcome to it. Exercises `awaiting_retry_needing_terminal` plus

#[test]
fn live_kill_override_turns_done_into_aborted_when_flagged() {
    let mut state = empty_state();
    let mut d = directive("d1", "coder-1");
    d.status = MiniCoderStatus::Running;
    d.kill_requested = true;
    state.mini_coder_directives.push(d);
    let done = MiniCoderOutcome::done(MiniCoderResult {
        status: "done".into(),
        ..Default::default()
    });
    let overridden = live_kill_override(&state, "d1", done);
    assert_eq!(overridden.status, MiniCoderStatus::AbortedByHuman);

    // Not flagged -> unchanged.
    state.mini_coder_directives[0].kill_requested = false;
    let done2 = MiniCoderOutcome::done(MiniCoderResult {
        status: "done".into(),
        ..Default::default()
    });
    let kept = live_kill_override(&state, "d1", done2);
    assert_eq!(kept.status, MiniCoderStatus::Done);
}

#[test]
fn live_kill_override_honors_stop_sentinel_in_steer_queue() {
    // ASYNC STEERING (a): a STOP sentinel that reached the live steer_queue out-of-band
    // (no kill_requested set) is still an abort at the round boundary — the SAME
    // external-signal channel generalized from the bool.
    let mut state = empty_state();
    let mut d = directive("d1", "coder-1");
    d.status = MiniCoderStatus::Running;
    d.kill_requested = false;
    d.steer_queue = vec!["stop".into()];
    state.mini_coder_directives.push(d);
    let done = MiniCoderOutcome::done(MiniCoderResult {
        status: "done".into(),
        ..Default::default()
    });
    let overridden = live_kill_override(&state, "d1", done);
    assert_eq!(overridden.status, MiniCoderStatus::AbortedByHuman);

    // A NON-stop steer message is NOT an abort (it is a queued correction, not a stop).
    state.mini_coder_directives[0].steer_queue = vec!["use a HashMap".into()];
    let done2 = MiniCoderOutcome::done(MiniCoderResult {
        status: "done".into(),
        ..Default::default()
    });
    let kept = live_kill_override(&state, "d1", done2);
    assert_eq!(kept.status, MiniCoderStatus::Done);
}
#[test]
// awaiting_retry_kill_window_aborts_chain_and_spawns_no_retry: deleted (gate Step 8)
#[test]
fn plan_result_file_sweep_keeps_live_drops_terminal_and_unknown() {
    // WARNING 5: the pure sweep plan keeps ONLY a live (non-terminal) directive's
    // result file in its scratch dir; a terminal directive's file is NOT kept (it
    // should have been deleted on finalize -> reclaim it), and a dir is keyed once.
    let scratch = "/proj/.aspis-mini";
    let mut live = directive("live1", "coder-1");
    live.status = MiniCoderStatus::Running;
    live.result_path = "live1.json".into();
    live.scratch_path = Some(scratch.to_string());

    let mut term = directive("term1", "coder-1");
    term.status = MiniCoderStatus::Done;
    term.result_path = "term1.json".into();
    term.scratch_path = Some(scratch.to_string());

    // A directive with no scratch path contributes nothing.
    let mut nodir = directive("nodir", "coder-1");
    nodir.status = MiniCoderStatus::Running;
    nodir.scratch_path = None;

    let plan = plan_result_file_sweep(&[live, term, nodir]);
    assert_eq!(plan.len(), 1, "one distinct scratch dir");
    let keep = plan.get(&PathBuf::from(scratch)).expect("dir present");
    assert!(keep.contains("live1.json"), "live result file kept");
    assert!(
        !keep.contains("term1.json"),
        "terminal result file NOT kept (reclaimed)"
    );
    // FIX 1: a live directive's in-flight `.raw` capture is also kept; a terminal
    // one's `.raw` is reclaimable.
    assert!(keep.contains("live1.json.raw"), "live raw capture kept");
    assert!(
        !keep.contains("term1.json.raw"),
        "terminal raw capture NOT kept (reclaimed)"
    );
}

#[test]
fn sweep_orphaned_result_files_plan_deletes_only_orphan_json() {
    // WARNING 5 (fs behavior): given a real scratch dir with a live file, an
    // orphan (terminal) file, and a non-json file, the plan keeps the live file and
    // marks the orphan for deletion; the non-json is untouched by construction
    // (the fn only ever removes `*.json`). We exercise the plan + the same
    // delete/keep predicate the fn uses.
    let dir = std::env::temp_dir().join(format!("mc_p5_sweep_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("live.json"), "{}").unwrap();
    std::fs::write(dir.join("orphan.json"), "{}").unwrap();
    std::fs::write(dir.join("keep.txt"), "x").unwrap();
    // FIX 1: a live directive's in-flight `.raw` capture must SURVIVE; a stray
    // orphan `.raw` (left by a hard-killed mini) must be reclaimed.
    std::fs::write(dir.join("live.json.raw"), "x").unwrap();
    std::fs::write(dir.join("orphan.json.raw"), "x").unwrap();

    let mut live = directive("live", "coder-1");
    live.status = MiniCoderStatus::Running;
    live.result_path = "live.json".into();
    live.scratch_path = Some(dir.to_string_lossy().to_string());

    let plan = plan_result_file_sweep(&[live]);
    let keep = plan.get(&dir).cloned().unwrap_or_default();

    // Apply the exact same predicate the fn applies (now incl. `.raw`).
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        let is_json = ext.is_some_and(|e| e.eq_ignore_ascii_case("json"));
        let is_raw = ext.is_some_and(|e| e.eq_ignore_ascii_case("raw"));
        if (!is_json && !is_raw) || !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .to_string();
        if keep.contains(&name) {
            continue;
        }
        let _ = std::fs::remove_file(&path);
    }

    assert!(dir.join("live.json").exists(), "live result file survives");
    assert!(!dir.join("orphan.json").exists(), "orphan json deleted");
    assert!(dir.join("keep.txt").exists(), "non-json untouched");
    assert!(
        dir.join("live.json.raw").exists(),
        "live raw capture survives"
    );
    assert!(
        !dir.join("orphan.json.raw").exists(),
        "orphan raw capture deleted"
    );
    std::fs::remove_dir_all(&dir).ok();
}

fn test_session(agent_id: &str, status: &str) -> crate::backend::model::AgentSession {
    crate::backend::model::AgentSession {
        agent_id: agent_id.into(),
        role: "coder".into(),
        model: None,
        status: status.into(),
        client: None,
        message: None,
        current_project_id: None,
        current_task_id: None,
        current_file_path: None,
        first_seen_at: None,
        last_seen_at: None,
        launch_token_hash: None,
        launch_token_issued_at: None,
        session_token_hash: None,
        session_token_issued_at: None,
        subagents: Vec::new(),
        needs_user: None,
        host: None,
        parent_agent_id: None,
        pending_question: None,
        user_reply: None,
    }
}

// -- P4: prompt + per-kind command build ---------------------------------

fn backend(
    kind: MiniCoderBackendKind,
    model: Option<&str>,
    command: Option<&str>,
) -> MiniCoderBackend {
    MiniCoderBackend {
        kind,
        model: model.map(|s| s.to_string()),
        command: command.map(|s| s.to_string()),
        base_url: None,
        max_concurrent: None,
    }
}

/// oMLX-P2 test helper: an omlx backend with a (normalized, loopback) base URL +
/// model, as oMLX-P1 validation would produce.
fn omlx_backend(model: &str, base_url: &str) -> MiniCoderBackend {
    MiniCoderBackend {
        kind: MiniCoderBackendKind::Omlx,
        model: Some(model.to_string()),
        command: None,
        base_url: Some(base_url.to_string()),
        max_concurrent: None,
    }
}

fn p4_directive(allow_oracle: bool) -> MiniCoderDirective {
    MiniCoderDirective {
        id: "d1".into(),
        parent_agent_id: "coder-1".into(),
        status: MiniCoderStatus::Running,
        task: "add a docstring to foo()".into(),
        files: vec!["src/a.rs".into(), "src/b.rs".into()],
        backend: None,
        write: false,
        write_mode: mini_coder::WriteMode::EmitEdits,
        tier: Default::default(),
        project_id: None,
        allow_oracle,
        kill_requested: false,
        steer_queue: Vec::new(),
        result_path: "d1.json".into(),
        agent_id: None,
        created_at: "2026-06-06T00:00:00Z".into(),
        claimed_at: None,
        scratch_path: None,
        started_at: None,
        result: None,
        attempt: 0,
        parent_directive_id: None,
        pigeon_ticket: None,
    }
}

#[test]
fn read_prompt_file_rejects_symlink_escaping_the_root() {
    // WARNING 3: a `files` entry inside the project root that is a SYMLINK to a
    // file OUTSIDE the root must NOT be front-loaded (canonicalize-after-join).
    let base = std::env::temp_dir().join(format!("mc_symlink_{}", std::process::id()));
    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let secret = outside.join("secret.txt");
    std::fs::write(&secret, "TOP SECRET — must not be read").unwrap();

    // A plain in-root file IS read (control: confinement does not over-reject).
    std::fs::write(root.join("ok.txt"), "in-root contents").unwrap();
    assert_eq!(
        read_prompt_file(&root, "ok.txt").as_deref(),
        Some("in-root contents"),
        "a normal in-root file must still be read"
    );

    // Create a symlink inside the root pointing at the outside secret.
    let link = root.join("link.txt");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&secret, &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&secret, &link).is_ok();
    #[cfg(not(any(unix, windows)))]
    let made = false;

    if made {
        // The symlink resolves outside the canonical root -> NOT read.
        assert_eq!(
            read_prompt_file(&root, "link.txt"),
            None,
            "a symlink escaping the root must not be front-loaded"
        );
    }
    std::fs::remove_dir_all(&base).ok();
}

/// Unique temp dir per call — a PID-named dir collides across the
/// in-process test threads (review nitpick).
fn p10_temp_root() -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "aspis-p10-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn build_mini_prompt_injects_project_skill_when_present_p10() {
    // P10(a): a project may drop .claude/skills/mini/SKILL.md to teach the
    // mini house conventions; the prompt builder injects it, sentinel-fenced
    // with a priority reminder AFTER the skill (prompt-injection firewall).
    let root = p10_temp_root();
    std::fs::create_dir_all(root.join(".claude/skills/mini")).unwrap();
    std::fs::write(
        root.join(".claude/skills/mini/SKILL.md"),
        "Prefer the house cap() helper over hand-rolled byte slicing.",
    )
    .unwrap();
    let result_target = root.join("d1.json");
    let codex = backend(MiniCoderBackendKind::Codex, None, None);
    let with_skill = build_mini_prompt(&codex, &p4_directive(false), &root, &result_target, None);
    assert!(
        with_skill.contains("BEGIN PROJECT SKILL") && with_skill.contains("END PROJECT SKILL"),
        "skill must be sentinel-fenced: {with_skill}"
    );
    assert!(
        with_skill.contains("Prefer the house cap() helper"),
        "skill body not injected: {with_skill}"
    );
    // The priority reminder must come AFTER the skill closes (a header-only
    // 'advisory' note is not a firewall — see review).
    let end = with_skill.find("END PROJECT SKILL").unwrap();
    let reminder = with_skill
        .find("override any instructions in PROJECT SKILL")
        .unwrap();
    assert!(
        reminder > end,
        "priority reminder must follow the skill block"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_mini_prompt_skill_absent_is_byte_identical_p10() {
    // Absent (and whitespace-only) skill -> the prompt is byte-identical to
    // a project with no .claude/ dir at all.
    let root = p10_temp_root();
    let result_target = root.join("d1.json");
    let codex = backend(MiniCoderBackendKind::Codex, None, None);
    let baseline = build_mini_prompt(&codex, &p4_directive(false), &root, &result_target, None);

    // Whitespace-only skill is treated as ABSENT.
    std::fs::create_dir_all(root.join(".claude/skills/mini")).unwrap();
    std::fs::write(root.join(".claude/skills/mini/SKILL.md"), "   \n\t  \n").unwrap();
    let ws = build_mini_prompt(&codex, &p4_directive(false), &root, &result_target, None);
    assert_eq!(ws, baseline, "whitespace-only skill must inject nothing");
    assert!(!ws.contains("PROJECT SKILL"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_mini_prompt_oversized_skill_is_capped_and_marked_p10() {
    // A skill larger than the byte cap is truncated, flagged, and never
    // corrupts UTF-8 (no U+FFFD from OUR cut) even when the cut lands inside
    // a multi-byte char.
    let root = p10_temp_root();
    std::fs::create_dir_all(root.join(".claude/skills/mini")).unwrap();
    // 3-byte chars (€) so the 8192-byte cut lands MID-char (8192 = 3*2730+2),
    // forcing the split a naive byte truncate would corrupt into U+FFFD.
    let big = "€".repeat(crate::backend::project_skill::MAX_SKILL_BYTES); // 3 * cap bytes
    std::fs::write(root.join(".claude/skills/mini/SKILL.md"), &big).unwrap();
    let result_target = root.join("d1.json");
    let codex = backend(MiniCoderBackendKind::Codex, None, None);
    let p = build_mini_prompt(&codex, &p4_directive(false), &root, &result_target, None);
    assert!(p.contains("(skill truncated)"), "oversize must be marked");
    assert!(
        !p.contains('\u{FFFD}'),
        "our cap must not introduce a replacement char"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_mini_prompt_has_constraints_file_scope_and_schema() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let codex = backend(MiniCoderBackendKind::Codex, None, None);
    let prompt = build_mini_prompt(&codex, &p4_directive(false), &root, &result_target, None);

    // The task + the explicit file scope are embedded.
    assert!(prompt.contains("add a docstring to foo()"), "task missing");
    assert!(prompt.contains("src/a.rs"), "file scope missing a.rs");
    assert!(prompt.contains("src/b.rs"), "file scope missing b.rs");
    // Anti-destructive constraints present.
    assert!(prompt.contains("rm -rf"), "anti-destructive block missing");
    assert!(
        prompt.contains("force-push"),
        "anti-destructive block missing"
    );
    assert!(
        prompt.contains("outside the FILE SCOPE"),
        "scope constraint missing"
    );
    assert!(
        prompt.contains("visual_check"),
        "visual-check handoff missing"
    );
    // Result schema present.
    assert!(
        prompt.contains("needs_clarification"),
        "schema status missing"
    );
    assert!(prompt.contains("filesTouched"), "schema field missing");
    // codex writes the file itself; the exact resultPath is named.
    assert!(
        prompt.contains(&result_target.to_string_lossy().to_string()),
        "resultPath missing"
    );
    assert!(
        prompt.contains("WRITE this JSON object to the file"),
        "codex write instruction missing"
    );
}

#[test]
fn build_mini_prompt_places_stable_blocks_before_volatile_task_fix4() {
    // FIX 4 (prompt cache-friendliness): the STABLE blocks (file-scope,
    // hard-constraints, result-contract) must all precede the VOLATILE TASK
    // block, so the mlx-lm/oMLX longest-stable-prefix cache survives the
    // write→fix retries (only the task tail changes per retry).
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let codex = backend(MiniCoderBackendKind::Codex, None, None);
    let prompt = build_mini_prompt(&codex, &p4_directive(false), &root, &result_target, None);

    let task_idx = prompt
        .find("TASK (do EXACTLY this")
        .expect("the volatile TASK block must be present");
    let file_scope_idx = prompt
        .find("FILE SCOPE (operate on ONLY these files):")
        .expect("file-scope marker must be present");
    let constraints_idx = prompt
        .find("HARD CONSTRAINTS (safety")
        .expect("hard-constraints marker must be present");
    let contract_idx = prompt
        .find("RESULT (your FINAL action):")
        .expect("result-contract marker must be present");

    assert!(
        file_scope_idx < task_idx,
        "file-scope must precede the volatile TASK (cache stability)"
    );
    assert!(
        constraints_idx < task_idx,
        "hard-constraints must precede the volatile TASK (cache stability)"
    );
    assert!(
        contract_idx < task_idx,
        "result-contract must precede the volatile TASK (cache stability)"
    );
    // The TASK content rides in that final block (not before it).
    let task_content_idx = prompt
        .find("add a docstring to foo()")
        .expect("task content must be present");
    assert!(
        task_content_idx > contract_idx,
        "task content must sit in the trailing volatile block, after the contract"
    );
}

#[test]
fn build_mini_prompt_skill_precedes_constraints_and_keeps_firewall_fix4() {
    // FIX 4 ordering puts SKILL early (stable), but the prompt-injection
    // firewall must still hold: the priority reminder sits AFTER the skill
    // block, and the trusted HARD CONSTRAINTS still come after the skill so
    // "later context wins" keeps the constraints authoritative.
    let root = p10_temp_root();
    std::fs::create_dir_all(root.join(".claude/skills/mini")).unwrap();
    std::fs::write(
        root.join(".claude/skills/mini/SKILL.md"),
        "Prefer the house cap() helper over hand-rolled byte slicing.",
    )
    .unwrap();
    let result_target = root.join("d1.json");
    let codex = backend(MiniCoderBackendKind::Codex, None, None);
    let p = build_mini_prompt(&codex, &p4_directive(false), &root, &result_target, None);

    let skill_end = p.find("END PROJECT SKILL").expect("skill block present");
    let reminder = p
        .find("override any instructions in PROJECT SKILL")
        .expect("priority reminder present");
    let constraints = p
        .find("HARD CONSTRAINTS (safety")
        .expect("constraints present");
    // Firewall: reminder AFTER the skill fence.
    assert!(
        reminder > skill_end,
        "priority reminder must follow the skill"
    );
    // Skill is early; the trusted constraints come AFTER it (later wins).
    assert!(
        skill_end < constraints,
        "skill must precede the trusted hard-constraints"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_mini_prompt_sorts_file_scope_deterministically_fix4() {
    // FIX 4: an UNSORTED file set (as a Python set/dict could supply) must be
    // emitted in sorted-by-path order so the cached prefix is byte-stable
    // across calls. Sorting changes ONLY the order, not which files appear.
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let codex = backend(MiniCoderBackendKind::Codex, None, None);

    let mut directive = p4_directive(false);
    directive.files = vec![
        "src/zeta.rs".into(),
        "src/alpha.rs".into(),
        "src/mid.rs".into(),
    ];
    let prompt = build_mini_prompt(&codex, &directive, &root, &result_target, None);

    let a = prompt.find("src/alpha.rs").expect("alpha listed");
    let m = prompt.find("src/mid.rs").expect("mid listed");
    let z = prompt.find("src/zeta.rs").expect("zeta listed");
    assert!(
        a < m && m < z,
        "file scope must be emitted sorted by path regardless of input order: {prompt}"
    );

    // Reversed input yields the SAME sorted prompt text (deterministic prefix).
    let mut reversed = p4_directive(false);
    reversed.files = vec![
        "src/mid.rs".into(),
        "src/zeta.rs".into(),
        "src/alpha.rs".into(),
    ];
    let prompt_rev = build_mini_prompt(&codex, &reversed, &root, &result_target, None);
    assert_eq!(
        prompt, prompt_rev,
        "different input file order must produce a byte-identical prompt"
    );
}

#[test]
fn build_mini_prompt_sort_decides_which_files_are_inlined_over_max_fix4() {
    // FIX A: when files.len() > MAX_PROMPT_FILES (20) the content-inlining loop
    // only inlines the FIRST MAX_PROMPT_FILES entries — and since FIX 4 sorts
    // first, that is the 20 *alphabetically-first* files, NOT the first 20 by
    // input order. Supplied here in REVERSE-alphabetical order: the deterministic
    // sort must still inline f00..f19 (alphabetically first) and list f20
    // (alphabetically last) by PATH ONLY. This pins the behavior FIX A documents.
    let root = p10_temp_root();
    // 21 zero-padded files so alphabetical order is unambiguous (f00 < .. < f20).
    // Each carries a unique sentinel so "content inlined" is detectable in the prompt.
    let names: Vec<String> = (0..=20).map(|i| format!("f{i:02}.rs")).collect();
    for name in &names {
        std::fs::write(root.join(name), format!("// SENTINEL_CONTENT_{name}\n")).unwrap();
    }
    // Supply the set in REVERSE-alphabetical input order (f20 first, f00 last).
    let mut directive = p4_directive(false);
    directive.files = names.iter().rev().cloned().collect();
    assert_eq!(
        directive.files.len(),
        21,
        "must exceed MAX_PROMPT_FILES (20)"
    );
    assert_eq!(directive.files[0], "f20.rs", "input order is reverse-alpha");

    let result_target = root.join("d1.json");
    let codex = backend(MiniCoderBackendKind::Codex, None, None);
    let prompt = build_mini_prompt(&codex, &directive, &root, &result_target, None);

    // The 20 alphabetically-first files (f00..f19) get their content INLINED.
    for i in 0..MAX_PROMPT_FILES {
        let sentinel = format!("SENTINEL_CONTENT_f{i:02}.rs");
        assert!(
            prompt.contains(&sentinel),
            "alphabetically-first file f{i:02}.rs must have its content inlined"
        );
    }
    // The 21st alphabetically (f20.rs) is listed by PATH ONLY — NOT inlined —
    // even though it was supplied FIRST in input order. The sort, not input
    // order, decides which files are inlined.
    assert!(
        prompt.contains("- f20.rs\n"),
        "f20.rs must still be listed by path"
    );
    assert!(
        !prompt.contains("SENTINEL_CONTENT_f20.rs"),
        "alphabetically-last file f20.rs must NOT be inlined despite leading input order"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_mini_prompt_oracle_grant_is_codex_only_p3() {
    // P3 (supersedes the MINOR 9 pin): a codex mini WITH the oracle access
    // advertises exactly ONE read-only tool (oracle_context) + the
    // register-first contract carrying its launch token. Without access —
    // or on a text-only backend even WITH access — the NO-tools contract
    // stands, so the grant can never leak past the codex kind gate.
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let access = MiniOracleAccess {
        agent_id: "mini-x-1",
        launch_token: "tok-3f9a",
    };

    let codex = backend(MiniCoderBackendKind::Codex, None, None);
    let p = build_mini_prompt(
        &codex,
        &p4_directive(true),
        &root,
        &result_target,
        Some(&access),
    );
    assert!(
        p.contains("oracle_context"),
        "granted codex must advertise oracle_context"
    );
    assert!(
        p.contains("agent_register") && p.contains("\"role\": \"mini\""),
        "register-first contract missing"
    );
    assert!(
        p.contains("tok-3f9a") && p.contains("mini-x-1"),
        "launch token / agent id missing from the grant text"
    );
    assert!(
        !p.contains("You have NO external tools"),
        "granted codex must not get the NO-tools contract"
    );
    // codex still WRITES the result file itself.
    assert!(
        p.contains("WRITE this JSON object to the file"),
        "codex write instruction missing"
    );

    // No access -> the NO-tools contract, even with allow_oracle on the directive.
    let p = build_mini_prompt(&codex, &p4_directive(true), &root, &result_target, None);
    assert!(
        !p.contains("oracle_context"),
        "no access must mean no oracle text"
    );
    assert!(
        p.contains("You have NO external tools"),
        "must tell the ungranted mini it has no tools"
    );

    // ollama is text-only: access is IGNORED (kind gate), and it must OUTPUT.
    let ollama = backend(MiniCoderBackendKind::Ollama, Some("qwen2.5-coder"), None);
    let p = build_mini_prompt(
        &ollama,
        &p4_directive(true),
        &root,
        &result_target,
        Some(&access),
    );
    assert!(
        !p.contains("oracle_context"),
        "ollama (text-only) must never advertise oracle, even with access"
    );
    assert!(
        p.contains("OUTPUT this JSON object to stdout"),
        "ollama must output, not write"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn codex_mini_command_wires_mcp_flags_only_with_roots_p3() {
    // P3: with roots the codex arm carries the shared `-c mcp_servers.*`
    // tokens (server-side "mini"-role narrowing); without roots the command
    // is byte-identical to the MINOR 9 status quo (no MCP flags at all).
    let root = std::env::temp_dir();
    let result_target = root.join("r.json");
    let prompt_file = root.join("p.txt");
    let codex = backend(MiniCoderBackendKind::Codex, None, None);
    let roots = McpRoots {
        management_root: root.clone(),
        projects_dir: root.join("projects"),
    };

    let with = build_mini_command_impl(
        &codex,
        &root,
        &result_target,
        &prompt_file,
        None,
        Some(&roots),
        false,
    )
    .expect("granted codex command builds")
    .0;
    let with_line = format!("{with:?}");
    assert!(
        with_line.contains("mcp_servers.aspis-management.command"),
        "granted codex must wire the MCP server flags"
    );

    let without = build_mini_command_impl(
        &codex,
        &root,
        &result_target,
        &prompt_file,
        None,
        None,
        false,
    )
    .expect("ungranted codex command builds")
    .0;
    let without_line = format!("{without:?}");
    assert!(
        !without_line.contains("mcp_servers"),
        "ungranted codex must carry NO MCP flags"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn mini_command_never_carries_user_mcp_env_even_when_host_has_it() {
    // FIX 6 (mini-exclusion, runtime): CommandBuilder::new() snapshots the HOST process
    // env. If the app was launched from a shell that already had
    // DEVBOULE_USER_MCP_SERVERS set, the mini child would otherwise INHERIT it. The
    // defensive `cmd.env_remove(FORBIDDEN_USER_MCP_ENV)` must strip it so the built mini
    // command never carries it. We SET it on this process's env, build the command, and
    // assert it is absent from the command's env (get_env reads the env map the child
    // would inherit). Env is process-global; this test owns + restores the exact var.
    let prev = std::env::var(FORBIDDEN_USER_MCP_ENV).ok();
    std::env::set_var(
        FORBIDDEN_USER_MCP_ENV,
        "[{\"name\":\"evil\",\"command\":\"x\"}]",
    );

    let root = std::env::temp_dir();
    let result_target = root.join("r.json");
    let prompt_file = root.join("p.txt");
    // An oMLX (local-loopback) backend builds a real command on macOS (the sandboxed arm).
    let b = omlx_backend("qwen2.5-coder", "http://127.0.0.1:8000/v1");
    let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .expect("oMLX mini command builds")
        .0;

    assert!(
        cmd.get_env(FORBIDDEN_USER_MCP_ENV).is_none(),
        "the mini command must NOT carry the user-MCP env var even when the host \
             env has it set (mini-exclusion §6, runtime env_remove via FORBIDDEN_USER_MCP_ENV)"
    );

    // Restore the host env regardless of the assertion outcome path above.
    match prev {
        Some(v) => std::env::set_var(FORBIDDEN_USER_MCP_ENV, v),
        None => std::env::remove_var(FORBIDDEN_USER_MCP_ENV),
    }
}

#[cfg(windows)]
fn argv_strings(cmd: &portable_pty::CommandBuilder) -> Vec<String> {
    cmd.get_argv()
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect()
}

#[cfg(windows)]
#[test]
fn build_command_applefm_windows_returns_clean_macos_only_error() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("fake-prompt.txt");
    let b = backend(MiniCoderBackendKind::AppleFm, Some("apple-model"), None);
    let err = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .unwrap_err();
    assert_eq!(err, "Apple on-device requires macOS 27+.");
}

#[test]
fn mini_thinking_directive_branches_by_family() {
    // Q1: Gemma + North-Mini (no thinking_budget param) get the in-prompt brevity directive.
    assert!(mini_thinking_directive(Some("gemma-4-26B-A4B-it-OptiQ-4bit")).contains("BRIEFLY"));
    assert!(mini_thinking_directive(Some("North-Mini-Code-1.0-4bit")).contains("BRIEFLY"));
    // Qwen (bounded via the enable_thinking param) + unknown + None get NO prompt line.
    assert_eq!(
        mini_thinking_directive(Some("Qwen3.6-35B-A3B-4bit-DWQ")),
        ""
    );
    assert_eq!(mini_thinking_directive(Some("llama3.1")), "");
    assert_eq!(mini_thinking_directive(None), "");
}

#[cfg(windows)]
#[test]
fn build_command_codex_uses_codex_exec_and_pipes_prompt_via_stdin() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("fake-prompt.txt");
    let b = backend(MiniCoderBackendKind::Codex, Some("gpt-5-codex"), None);
    // No mcp_roots -> no oracle grant flags.
    let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .unwrap()
        .0;
    let argv = argv_strings(&cmd);
    assert_eq!(argv[0], "powershell.exe");
    let script = argv.last().unwrap();
    // codex exec with the model, prompt piped over stdin.
    assert!(script.contains("'exec'"), "codex exec missing: {script}");
    assert!(
        script.contains("'-m', 'gpt-5-codex'"),
        "model flag missing: {script}"
    );
    assert!(
        script.contains("$prompt | & codex @codexArgs"),
        "stdin pipe missing: {script}"
    );
    // NO MCP -c flags when oracle not granted.
    assert!(
        !script.contains("mcp_servers.aspis-management"),
        "oracle grant leaked: {script}"
    );
    // B1: the prompt is read from the file then DELETED; never Write-Host'd.
    assert!(
        script.contains("Get-Content -Raw -LiteralPath $promptFile"),
        "prompt-file read missing"
    );
    assert!(
        !script.contains("Write-Host $prompt"),
        "prompt must never be echoed to the PTY"
    );
    // FIX 1: cleanup of the source-bearing prompt dir + raw capture lives in a
    // `finally` so it ALWAYS runs (even if Get-Content / the backend errors).
    assert!(
        script.contains("finally {"),
        "finally cleanup block missing: {script}"
    );
    assert!(
        script.contains("Remove-Item -LiteralPath $promptDir -Recurse -Force"),
        "prompt dir cleanup must be in finally: {script}"
    );
    // F5: codex writes NO `.raw` file (it does not use the stdout wrapper), so the
    // raw-file removal is guarded by Test-Path — it never runs Remove-Item on a
    // non-existent file.
    assert!(
        script.contains(
            "if (Test-Path -LiteralPath $rawFile) { Remove-Item -LiteralPath $rawFile -Force"
        ),
        "raw capture removal must be Test-Path-guarded: {script}"
    );
    // F4: a non-keyed backend (codex here) carries NO oMLX key-cleanup collateral.
    assert!(
        !script.contains("$env:OMLX_KEY_FILE"),
        "non-keyed codex script must not carry the oMLX key cleanup: {script}"
    );
    // P5 test 9 (windows_mini_command_unchanged): Windows is NOT sandboxed this phase.
    // The program is powershell.exe (NOT sandbox-exec), no `.sb` profile is emitted,
    // and the script carries none of the macOS-only sandbox/rlimit collateral.
    assert_eq!(
        argv[0], "powershell.exe",
        "Windows must spawn powershell directly"
    );
    assert!(
        !argv.iter().any(|a| a.contains("sandbox-exec")),
        "Windows argv must never reference sandbox-exec: {argv:?}"
    );
    assert!(
        !script.contains("sandbox-exec") && !script.contains("ulimit -"),
        "Windows script must carry no sandbox-exec / ulimit collateral: {script}"
    );
}

// ---- oMLX-P2 (Windows launch script) -----------------------------------

#[cfg(windows)]
#[test]
fn build_command_omlx_windows_posts_chat_completions_via_rest() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p").join("fake-prompt.txt");
    let b = omlx_backend("qwen2.5-coder", "http://localhost:8000/v1");
    let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .unwrap()
        .0;
    let argv = argv_strings(&cmd);
    assert_eq!(argv[0], "powershell.exe");
    let script = argv.last().unwrap();

    // POSTs via Invoke-RestMethod to <base>/chat/completions (no double slash).
    assert!(
        script.contains("Invoke-RestMethod -Method Post"),
        "must POST via Invoke-RestMethod: {script}"
    );
    // F3: a -TimeoutSec cap (derived from DEFAULT_WALL_CLOCK_CAP_SECS minus a margin)
    // makes a stalled server fail fast instead of holding the PTY to the wall-clock
    // kill. On timeout Invoke-RestMethod throws -> the try/catch yields the clean
    // failed fallback.
    let expected_timeout =
        super::mini_coder::DEFAULT_WALL_CLOCK_CAP_SECS - OMLX_HTTP_TIMEOUT_MARGIN_SECS;
    assert!(
        script.contains(&format!("-TimeoutSec {expected_timeout}")),
        "Invoke-RestMethod must carry a -TimeoutSec derived from the wall-clock cap: {script}"
    );
    assert!(
        script.contains("'http://localhost:8000/v1/chat/completions'"),
        "must target <base>/chat/completions, quoted, no double slash: {script}"
    );
    assert!(
        !script.contains("/v1//chat") && !script.contains(".0/chat//"),
        "no double slash in the URI: {script}"
    );
    // Body built by the ConvertTo-Json ENCODER (never string-concatenated).
    assert!(
        script.contains("| ConvertTo-Json -Depth 6 -Compress"),
        "body must be JSON-encoded by ConvertTo-Json: {script}"
    );
    assert!(
        script.contains("content = $prompt"),
        "prompt must ride as a VALUE encoded by ConvertTo-Json: {script}"
    );
    // INJECTION-SAFETY: the prompt is NEVER string-concatenated into the JSON body.
    assert!(
        !script.contains("\"content\":\"' +")
            && !script.contains("'+ $prompt")
            && !script.contains("+ $prompt +"),
        "prompt must NOT be concatenated into the JSON: {script}"
    );
    assert!(
        script.contains("temperature = 0.1") && script.contains("stream = $false"),
        "OpenAI envelope fields missing: {script}"
    );
    // FIX 2: the decode is BOUNDED — a hard max_tokens budget (the runaway guard on
    // this stream:false path) plus a repetition_penalty, both carrying the NAMED
    // constant values (no magic literals buried in the body string).
    assert!(
        script.contains(&format!("max_tokens = {OMLX_MAX_TOKENS_DEFAULT}")),
        "max_tokens must ride the body with the constant value: {script}"
    );
    assert!(
        script.contains(&format!("repetition_penalty = {OMLX_REPETITION_PENALTY}")),
        "repetition_penalty must ride the body with the constant value: {script}"
    );
    // P6 thinking split: this command was built with fix_pass_thinking=false
    // (an INITIAL write), so the Qwen-gated kwargs must say $false; a FIX
    // pass flips it to $true (pinned separately below).
    assert!(
        script.contains("-match 'qwen'")
            && script.contains("chat_template_kwargs")
            && script.contains("enable_thinking = $false")
            && script.contains("$body = $bodyMap | ConvertTo-Json -Depth 6 -Compress"),
        "Qwen-gated chat_template_kwargs missing from PS body: {script}"
    );
    // The fix-pass variant carries thinking ON.
    let fix_run = build_omlx_run_windows("http://localhost:8000/v1", "qwen2.5-coder", None, true);
    assert!(
        fix_run.contains("enable_thinking = $true"),
        "fix pass must enable thinking: {fix_run}"
    );
    // Extracts the model's content and writes it to stdout for the wrapper.
    assert!(
        script.contains("$resp.choices[0].message.content"),
        "must extract choices[0].message.content: {script}"
    );
    assert!(
        script.contains("Write-Output $content"),
        "content must be written to stdout: {script}"
    );
    // FAILURE = SILENCE: the request is wrapped in try/catch so any HTTP/parse error
    // writes nothing -> the wrapper writes the failed fallback.
    assert!(
        script.contains("try {") && script.contains("} catch { }"),
        "request must be wrapped in try/catch: {script}"
    );
    // Still feeds the EXISTING result-file write wrapper (balanced walk + write).
    assert!(
        script.contains("> $rawFile 2>$null"),
        "must feed the shared stdout->result wrapper: {script}"
    );
    assert!(
        script.contains("[System.IO.File]::WriteAllText"),
        "result-file write (the wrapper) must run: {script}"
    );
    assert!(
        script.contains("\\\"status\\\":\\\"failed\\\"") || script.contains("status\":\"failed"),
        "the wrapper's failed fallback must be present: {script}"
    );
    // No model on argv-visible token issues; model is OUR validated bare tag, quoted.
    assert!(
        script.contains("model = 'qwen2.5-coder'"),
        "model must be the configured tag: {script}"
    );
}

#[cfg(windows)]
#[test]
fn build_command_omlx_windows_no_key_emits_no_auth_header_and_no_key_env() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p").join("fake-prompt.txt");
    let b = omlx_backend("m", "http://127.0.0.1:8000");
    // No key file passed (the default; omlx_api_key returns None today).
    let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .unwrap()
        .0;
    let argv = argv_strings(&cmd);
    let script = argv.last().unwrap();
    // No auth header construction anywhere (no key configured).
    assert!(
        !script.contains("Authorization"),
        "no auth header without a key: {script}"
    );
    // F4: a non-keyed spawn carries NO key-env collateral ANYWHERE — neither the
    // request body nor the shared `finally` reference `$env:OMLX_KEY_FILE` (the
    // key-dir cleanup line is emitted only when a key is configured for this spawn).
    assert!(
        !script.contains("$env:OMLX_KEY_FILE"),
        "non-keyed script must not reference the key env anywhere: {script}"
    );
    // The key file env must NOT be set on the command when there is no key.
    assert!(
        cmd.get_env("OMLX_KEY_FILE").is_none(),
        "OMLX_KEY_FILE env must be absent without a key"
    );
    // F5: the raw-file removal is guarded by Test-Path (codex writes no raw file; here
    // the wrapper does, but the guard is uniform and harmless).
    assert!(
        script.contains("if (Test-Path -LiteralPath $rawFile) { Remove-Item -LiteralPath $rawFile"),
        "raw-file removal must be Test-Path-guarded: {script}"
    );
}

#[cfg(windows)]
#[test]
fn build_command_omlx_windows_with_key_rides_env_file_not_argv_and_cleans_up() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p").join("fake-prompt.txt");
    let key_file = root.join("kdir").join("omlx-key.txt");
    let b = omlx_backend("m", "http://localhost:8000/v1");
    let cmd = build_mini_command_impl(
        &b,
        &root,
        &result_target,
        &prompt_file,
        Some(&key_file),
        None,
        false,
    )
    .unwrap()
    .0;
    let argv = argv_strings(&cmd);
    let script = argv.last().unwrap();

    // The token is read from the env-passed FILE and sent as a Bearer header.
    assert!(
        script.contains("$env:OMLX_KEY_FILE")
            && script.contains("Get-Content -Raw -LiteralPath $env:OMLX_KEY_FILE"),
        "key must be read from the env-passed file: {script}"
    );
    assert!(
        script.contains("'Bearer ' + $omlxKey"),
        "token must ride an Authorization: Bearer header: {script}"
    );
    // max-recall FIX 8: the key variable is zeroed right after the header is set so the
    // token does not linger in PS scope for the rest of the script.
    assert!(
        script.contains("$omlxKey = $null"),
        "key variable must be zeroed after the header is set: {script}"
    );
    // The KEY FILE PATH rides on env, NEVER on argv. No argv entry contains the path.
    let key_str = key_file.to_string_lossy().to_string();
    assert!(
        !argv.iter().any(|a| a.contains(&key_str)),
        "key file path must NOT appear on argv: {argv:?}"
    );
    // The env var IS set on the command (path only — the token itself stays in the
    // file, never in env/argv).
    let env_val = cmd
        .get_env("OMLX_KEY_FILE")
        .map(|v| v.to_string_lossy().to_string());
    assert_eq!(
        env_val.as_deref(),
        Some(key_str.as_str()),
        "OMLX_KEY_FILE env must carry the key file PATH"
    );
    // The finally removes the key file's restricted dir on every exit path.
    assert!(
            script.contains("if ($env:OMLX_KEY_FILE) { Remove-Item -LiteralPath ([System.IO.Path]::GetDirectoryName($env:OMLX_KEY_FILE)) -Recurse -Force"),
            "finally must remove the key dir on every exit: {script}"
        );
}

#[cfg(windows)]
#[test]
fn build_command_windows_cleanup_always_runs_in_finally() {
    // FIX 1 (source-content leak): the read of the prompt happens INSIDE the try,
    // and the prompt dir + raw file are removed in the finally — so a failing
    // Get-Content can no longer skip cleanup and leak the front-loaded source.
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p").join("fake-prompt.txt");
    let b = backend(MiniCoderBackendKind::Ollama, Some("qwen2.5-coder"), None);
    let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .unwrap()
        .0;
    let script = argv_strings(&cmd).pop().unwrap();
    // The prompt read is inside the try (the try opens before Get-Content).
    let try_idx = script.find("try {").expect("try block");
    let read_idx = script
        .find("Get-Content -Raw -LiteralPath $promptFile")
        .expect("read");
    let finally_idx = script.find("finally {").expect("finally block");
    assert!(
        try_idx < read_idx,
        "Get-Content must be inside the try: {script}"
    );
    assert!(
        read_idx < finally_idx,
        "finally must come after the body: {script}"
    );
    // Both the prompt dir and the raw file are torn down in the finally.
    let finally_tail = &script[finally_idx..];
    assert!(
        finally_tail.contains("$promptDir"),
        "promptDir not cleaned in finally: {script}"
    );
    assert!(
        finally_tail.contains("$rawFile"),
        "rawFile not cleaned in finally: {script}"
    );
}

// FIX 1 (behavioral, Windows): run the REAL script with a backend that ERRORS
// and prove the source-bearing prompt dir + the .raw capture are gone afterward.
#[cfg(windows)]
#[test]
fn windows_finally_cleans_files_even_when_backend_errors() {
    use std::process::Command;
    let scratch = std::env::temp_dir().join(format!("mc_fix1win_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let prompt_dir = scratch.join("prompt");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    let prompt_file = prompt_dir.join("p.txt");
    std::fs::write(&prompt_file, "secret source code\n").unwrap();
    let result_target = scratch.join("d1.json");
    // An api command that EXITS NON-ZERO / errors (a non-existent executable). The
    // body throws under ErrorActionPreference=Stop, but the finally must still run.
    let b = backend(
        MiniCoderBackendKind::Api,
        None,
        Some("this_executable_does_not_exist_xyz"),
    );
    let cmd = build_mini_command_impl(
        &b,
        &scratch,
        &result_target,
        &prompt_file,
        None,
        None,
        false,
    )
    .unwrap()
    .0;
    let script = argv_strings(&cmd).pop().unwrap();
    let _ = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .current_dir(&scratch)
        .status()
        .expect("run script");
    assert!(
        !prompt_dir.exists(),
        "prompt dir must be removed by finally even on error"
    );
    let raw = scratch.join("d1.json.raw");
    assert!(
        !raw.exists(),
        "raw capture must be removed by finally even on error"
    );
    std::fs::remove_dir_all(&scratch).ok();
}

#[cfg(windows)]
#[test]
fn build_command_codex_never_adds_mcp_config_flags_even_with_roots() {
    // MINOR 9: a mini gets NO MCP grant. Even when McpRoots are supplied (the
    // plumbing is kept for a future read-only oracle scope), build_mini_command_impl
    // must NOT emit any `-c mcp_servers...` flags — the mini works from front-loaded
    // context only, never the full mutation-capable aspis-management server.
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("fake-prompt.txt");
    let b = backend(MiniCoderBackendKind::Codex, None, None);
    let roots = McpRoots {
        management_root: PathBuf::from("C:/mgmt"),
        projects_dir: PathBuf::from("C:/mgmt/projects"),
    };
    let cmd = build_mini_command_impl(
        &b,
        &root,
        &result_target,
        &prompt_file,
        None,
        Some(&roots),
        false,
    )
    .unwrap()
    .0;
    let script = argv_strings(&cmd).pop().unwrap();
    assert!(
        !script.contains("mcp_servers"),
        "mini must never get an MCP grant: {script}"
    );
    assert!(
        !script.contains("'-c'"),
        "mini must never get a -c flag: {script}"
    );
}

#[cfg(windows)]
#[test]
fn build_mini_command_wires_mcp_when_roots_present_windows() {
    // MINOR 9 → P3 at the public boundary: given McpRoots, the public
    // build_mini_command now WIRES the narrow MCP grant (server-side "mini"
    // role narrowing); without roots there are no MCP flags at all.
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let b = backend(MiniCoderBackendKind::Codex, None, None);
    let directive = p4_directive(true); // allow_oracle = true
    let prompt = build_mini_prompt(&b, &directive, &root, &result_target, None);
    let roots = McpRoots {
        management_root: PathBuf::from("C:/mgmt"),
        projects_dir: PathBuf::from("C:/mgmt/projects"),
    };
    let build =
        build_mini_command(&b, &root, &result_target, &prompt, Some(&roots), false).unwrap();
    let script = argv_strings(&build.command).pop().unwrap();
    assert!(
        script.contains("mcp_servers.aspis-management.command"),
        "granted mini must wire the MCP flags: {script}"
    );
    super::super::projects::remove_restricted_temp_file(&build.prompt_file.unwrap());

    let build = build_mini_command(&b, &root, &result_target, &prompt, None, false).unwrap();
    let script = argv_strings(&build.command).pop().unwrap();
    assert!(
        !script.contains("mcp_servers"),
        "ungranted mini must carry no MCP flags: {script}"
    );
    super::super::projects::remove_restricted_temp_file(&build.prompt_file.unwrap());
}

#[test]
fn remove_mini_temp_files_removes_prompt_key_and_profile_files() {
    // max-recall FIX 10 + P5: a spawn-failure cleanup must remove ALL restricted temp
    // files (prompt, the oMLX key file, AND the P5 Seatbelt `.sb` profile), each in its
    // OWN 0600 dir. A leaked `.sb` per launch is a bug. We create three real restricted
    // temp files (mirroring what build_mini_command writes) and assert the cleanup
    // removes all three files AND their dirs.
    let prompt_file = super::super::projects::write_restricted_prompt_file("prompt body")
        .expect("prompt file created");
    let key_file = super::super::projects::write_restricted_prompt_file("secret-token")
        .expect("key file created");
    let profile_file = super::super::projects::write_restricted_prompt_file("(version 1)")
        .expect("profile file created");
    // Distinct restricted directories (each call makes a fresh per-launch *.d dir).
    let prompt_dir = prompt_file.parent().unwrap().to_path_buf();
    let key_dir = key_file.parent().unwrap().to_path_buf();
    let profile_dir = profile_file.parent().unwrap().to_path_buf();
    assert!(prompt_dir != key_dir && key_dir != profile_dir && prompt_dir != profile_dir);
    assert!(prompt_file.exists() && key_file.exists() && profile_file.exists());

    remove_mini_temp_files(Some(&prompt_file), Some(&key_file), Some(&profile_file));

    assert!(!prompt_file.exists(), "prompt file must be removed");
    assert!(!key_file.exists(), "key file must be removed (no leak)");
    assert!(
        !profile_file.exists(),
        "profile .sb file must be removed (no leak)"
    );
    assert!(!prompt_dir.exists(), "prompt dir must be removed");
    assert!(!key_dir.exists(), "key dir must be removed (no leak)");
    assert!(
        !profile_dir.exists(),
        "profile dir must be removed (no leak)"
    );
}

#[cfg(windows)]
#[test]
fn build_command_ollama_runs_model_pipes_stdin_and_wraps_stdout() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("fake-prompt.txt");
    let b = backend(MiniCoderBackendKind::Ollama, Some("qwen2.5-coder"), None);
    let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .unwrap()
        .0;
    let script = argv_strings(&cmd).pop().unwrap();
    assert!(
        script.contains("ollama run 'qwen2.5-coder'"),
        "ollama run missing: {script}"
    );
    assert!(
        script.contains("$prompt | & ollama"),
        "stdin pipe missing: {script}"
    );
    // The stdout->result-file wrapper writes the normalized result.
    assert!(
        script.contains("ConvertFrom-Json"),
        "stdout wrapper missing: {script}"
    );
    assert!(
        script.contains("WriteAllText"),
        "result-file write missing: {script}"
    );
    // WARNING 7: stdout is redirected to a temp file, read bounded.
    assert!(
        script.contains("$rawFile"),
        "raw stdout temp file missing: {script}"
    );
    assert!(
        script.contains("StreamReader"),
        "bounded raw read missing: {script}"
    );
    // BLOCKER 2: balanced-brace walk (not first-{/last-}).
    assert!(
        script.contains("$depth"),
        "balanced-brace walk missing: {script}"
    );
    // No MCP/oracle for text-only ollama.
    assert!(
        !script.contains("mcp_servers"),
        "ollama must not get MCP: {script}"
    );
    // WARNING 8: the prompt-file parent restricted dir is removed too.
    assert!(
        script.contains("Remove-Item -LiteralPath $promptDir"),
        "parent dir cleanup missing: {script}"
    );
}

#[cfg(windows)]
#[test]
fn build_command_api_runs_configured_command_and_keeps_key_off_argv() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("fake-prompt.txt");
    // The user's CLI command. Any API key must come from the CLI's OWN env, not
    // from us — we never inject a key, so it can't be on argv.
    let b = backend(MiniCoderBackendKind::Api, None, Some("mycli chat --json"));
    let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .unwrap()
        .0;
    let argv = argv_strings(&cmd);
    let script = argv.last().unwrap();
    // BLOCKER 1 / WARNING 5: the multi-word command is piped to WITHOUT the `&`
    // call operator, so PowerShell tokenizes `mycli chat --json` itself (running
    // `mycli` with args `chat --json`). `& {command}` would treat the whole string
    // as a single executable name and fail.
    assert!(
        script.contains("$prompt | mycli chat --json"),
        "command must tokenize natively (no &): {script}"
    );
    assert!(
        !script.contains("$prompt | & mycli"),
        "must NOT use the & call operator on a command line: {script}"
    );
    assert!(
        script.contains("WriteAllText"),
        "stdout wrapper missing: {script}"
    );
    // B1: no secret token anywhere on argv (we never place one). The whole argv
    // joined must not contain an env-style key marker we'd inject.
    let joined = argv.join(" ");
    assert!(
        !joined.contains("API_KEY="),
        "a key must never be put on argv: {joined}"
    );
    assert!(
        !joined.contains("Authorization"),
        "no auth header on argv: {joined}"
    );
}

// BLOCKER (macOS trap): the EXIT trap must use DOUBLE-QUOTED shell variables, not
// the raw single-quoted paths, so a path containing whitespace (common on macOS,
// e.g. `/Users/the owner/My Project/`) does not terminate the trap's own single-quoted
// delimiter and turn the trap into a syntax error (which would leave it unarmed and
// leak the source-bearing prompt dir + `.raw` capture). This targets the pure,
// platform-agnostic `build_macos_trap_preamble`, so it runs on the Windows dev host.
#[test]
fn macos_trap_preamble_uses_quoted_vars_and_survives_spaces() {
    // Mirror sh_single_quote_local: wrap in single quotes, escape embedded quotes.
    fn q(v: &str) -> String {
        format!("'{}'", v.replace('\'', "'\\''"))
    }
    let prompt_dir = q("/Users/the owner/My Project/.aspis-mini-xyz");
    let raw_path = q("/Users/the owner/My Project/scratch/d1.json.raw");
    // No key configured here (the common case): key dir is None. Not sandboxed (codex/
    // api/non-loopback path) so no profile dir and no rlimits — the pre-P5 status quo.
    let preamble = build_macos_trap_preamble(&prompt_dir, &raw_path, None, None, false);

    // The paths are assigned to shell variables first.
    assert!(
        preamble.contains(&format!("_MINI_PROMPT_DIR={prompt_dir}")),
        "prompt dir must be assigned to a var with the quoted RHS: {preamble}"
    );
    assert!(
        preamble.contains(&format!("_MINI_RAW_FILE={raw_path}")),
        "raw path must always be assigned to a var with the quoted RHS: {preamble}"
    );
    // No-key case still assigns an empty _MINI_KEY_DIR so the trap body is fixed.
    assert!(
        preamble.contains("_MINI_KEY_DIR=''\n"),
        "no-key case must assign an empty key dir: {preamble}"
    );
    // P5: not sandboxed -> NO profile-dir machinery at all and NO rlimits (the preamble
    // is byte-for-byte the pre-P5 status quo).
    assert!(
        !preamble.contains("_MINI_PROFILE_DIR"),
        "non-sandboxed path must carry no profile-dir machinery: {preamble}"
    );
    assert!(
        !preamble.contains("ulimit -"),
        "non-sandboxed path must carry no rlimit lines: {preamble}"
    );

    // The trap body references DOUBLE-QUOTED variables, NOT the raw quoted paths. The
    // key-dir removal is GUARDED on a non-empty value (max-recall FIX 9) so the no-key
    // case never runs `rm -rf ""`; the prompt-dir/raw-file removal is unconditional. The
    // non-sandboxed trap is byte-identical to pre-P5 (no profile clause).
    assert!(
            preamble.contains(
                "trap 'rm -rf \"$_MINI_PROMPT_DIR\" \"$_MINI_RAW_FILE\" 2>/dev/null || true; [ -n \"$_MINI_KEY_DIR\" ] && rm -rf \"$_MINI_KEY_DIR\" 2>/dev/null || true' EXIT"
            ),
            "trap must reference double-quoted vars and guard the key-dir removal (pre-P5 string): {preamble}"
        );

    // The trap is armed BEFORE `set -e` (so it fires even on a set -e abort).
    let trap_idx = preamble.find("trap '").expect("trap present");
    let set_e_idx = preamble.find("\nset -e").expect("set -e present");
    assert!(trap_idx < set_e_idx, "trap must precede set -e: {preamble}");

    // The space-containing path must NOT appear literally inside the trap body —
    // only the variable expansion does. Isolate the trap line and check it.
    let trap_line_start = trap_idx;
    let trap_line_end = preamble[trap_line_start..]
        .find('\n')
        .map(|o| trap_line_start + o)
        .unwrap_or(preamble.len());
    let trap_line = &preamble[trap_line_start..trap_line_end];
    assert!(
        !trap_line.contains("My Project"),
        "the literal (space-containing) path must not appear in the trap body: {trap_line}"
    );
    assert!(
        !trap_line.contains(prompt_dir.as_str()),
        "the raw single-quoted prompt dir must not be embedded in the trap: {trap_line}"
    );
    assert!(
        !trap_line.contains(raw_path.as_str()),
        "the raw single-quoted raw path must not be embedded in the trap: {trap_line}"
    );
}

// ---- oMLX-P2 (macOS launch script — platform-agnostic source-text) ------
// These target the PURE `build_omlx_run_macos` / `build_macos_trap_preamble`, so
// they run on the Windows dev host (the macOS cargo target cannot build here).

#[test]
fn omlx_macos_run_posts_via_python_urllib_json_dumps_env_only() {
    // prompt_path arrives sh-quoted (as the macOS arm passes it).
    let prompt_q = "'/tmp/aspis-agent-prompt-abc.d/p.txt'";
    let run = build_omlx_run_macos(
        "http://localhost:8000/v1",
        "qwen2.5-coder",
        prompt_q,
        false,
        false,
    );

    // stdlib python3 + urllib, NO curl/jq.
    assert!(
        run.contains("python3 - <<'OMLXEOF'"),
        "must use a python3 heredoc: {run}"
    );
    assert!(
        run.contains("import urllib.request"),
        "must use stdlib urllib: {run}"
    );
    assert!(
        !run.contains("curl") && !run.contains("jq "),
        "must not shell out to curl/jq: {run}"
    );
    // Body via json.dumps ENCODER (injection-safe), prompt as a VALUE.
    assert!(
        run.contains("json.dumps("),
        "body must be json.dumps-encoded: {run}"
    );
    assert!(
        run.contains("'content': prompt"),
        "prompt must ride as a json.dumps VALUE: {run}"
    );
    // INJECTION-SAFETY: prompt is NOT string-concatenated into JSON.
    assert!(
        !run.contains("'\"content\":\"' +") && !run.contains("+ prompt +"),
        "prompt must NOT be concatenated into the JSON body: {run}"
    );
    assert!(
        run.contains("'temperature': 0.1") && run.contains("'stream': False"),
        "OpenAI envelope fields missing: {run}"
    );
    // FIX 2: the decode is BOUNDED — a hard max_tokens budget (the runaway guard on
    // this stream:false path) plus a repetition_penalty, both carrying the NAMED
    // constant values (no magic literals buried in the body string).
    assert!(
        run.contains(&format!("'max_tokens': {OMLX_MAX_TOKENS_DEFAULT}")),
        "max_tokens must ride the body with the constant value: {run}"
    );
    assert!(
        run.contains(&format!("'repetition_penalty': {OMLX_REPETITION_PENALTY}")),
        "repetition_penalty must ride the body with the constant value: {run}"
    );
    // P6 thinking split: built with fix_pass_thinking=false (INITIAL write)
    // -> False; a FIX pass flips the substituted placeholder to True.
    assert!(
        run.contains("'qwen' in model.lower()")
            && run.contains("body_dict['chat_template_kwargs'] = {'enable_thinking': False}"),
        "Qwen-gated chat_template_kwargs missing from python body: {run}"
    );
    let fix_run = build_omlx_run_macos(
        "http://localhost:8000/v1",
        "qwen2.5-coder",
        prompt_q,
        false,
        true,
    );
    assert!(
        fix_run.contains("{'enable_thinking': True}"),
        "fix pass must enable thinking: {fix_run}"
    );
    // base URL + prompt path ride via ENV (never argv).
    assert!(
        run.contains("OMLX_URL='http://localhost:8000/v1/chat/completions'"),
        "base URL must be exported via env, /chat/completions appended, no double slash: {run}"
    );
    assert!(
        run.contains("MINI_PROMPT_FILE='/tmp/aspis-agent-prompt-abc.d/p.txt'")
            && run.contains("os.environ['MINI_PROMPT_FILE']"),
        "prompt path must ride env MINI_PROMPT_FILE, read in python: {run}"
    );
    assert!(
        run.contains("urllib.request.Request(os.environ['OMLX_URL']")
            && run.contains("method='POST'"),
        "must POST to the env-passed URL: {run}"
    );
    // F2: the HTTP timeout is NOT hardcoded — it rides the OMLX_TIMEOUT env (derived
    // from DEFAULT_WALL_CLOCK_CAP_SECS minus a margin) and python reads it with a
    // matching default, so the two can never silently diverge.
    let expected_timeout =
        super::mini_coder::DEFAULT_WALL_CLOCK_CAP_SECS - OMLX_HTTP_TIMEOUT_MARGIN_SECS;
    assert!(
        run.contains(&format!(
            "OMLX_TIMEOUT={expected_timeout}\nexport OMLX_TIMEOUT"
        )),
        "HTTP timeout must be exported via env, derived from the wall-clock cap: {run}"
    );
    assert!(
        run.contains(&format!(
            "timeout = int(os.environ.get('OMLX_TIMEOUT', '{expected_timeout}'))"
        )) && run.contains("urlopen(req, timeout=timeout)"),
        "python must read the env timeout (not a hardcoded 600): {run}"
    );
    assert!(
        !run.contains("timeout=600"),
        "the hardcoded urlopen timeout must be gone: {run}"
    );
    // extracts choices[0].message.content, prints to stdout for the wrapper.
    assert!(
        run.contains("data['choices'][0]['message']['content']"),
        "must extract choices[0].message.content: {run}"
    );
    assert!(
        run.contains("sys.stdout.write(content)"),
        "content to stdout: {run}"
    );
    // FAILURE = SILENCE: any exception prints nothing (the outer try wraps the whole
    // request; its handler is a bare `pass`, so a non-2xx HTTPError / refused
    // connection / missing field writes no stdout and the wrapper emits the failed
    // fallback). Check the handler + its bare `pass` body independently of exact
    // whitespace.
    assert!(
        run.contains("except Exception:"),
        "the request must be wrapped in a catch-all except: {run}"
    );
    assert!(
        run.trim_end().ends_with("pass\nOMLXEOF") || run.contains("\n    pass\n"),
        "the catch-all handler must be a bare pass (no stdout on error): {run}"
    );
    // No-key case: key env cleared, no Authorization unless a key path is present.
    assert!(
        run.contains("unset OMLX_KEY_FILE"),
        "no-key case must clear the key env: {run}"
    );
}

#[test]
fn omlx_macos_run_with_key_reads_env_file_and_sends_bearer() {
    let prompt_q = "'/tmp/p.d/p.txt'";
    let run = build_omlx_run_macos("http://127.0.0.1:8000", "m", prompt_q, true, false);
    // The key path rides env; python reads the FILE and sends a Bearer header.
    assert!(
        run.contains("export OMLX_KEY_FILE"),
        "key env must be exported when keyed: {run}"
    );
    assert!(
        run.contains("key_path = os.environ.get('OMLX_KEY_FILE')")
            && run.contains("with open(key_path"),
        "token must be read from the env-passed key file: {run}"
    );
    assert!(
        run.contains("req.add_header('Authorization', 'Bearer ' + token)"),
        "token must ride an Authorization: Bearer header: {run}"
    );
    // The token VALUE never appears literally — only the file is read.
    assert!(
        !run.contains("Bearer sk-"),
        "no literal token in the script: {run}"
    );
}

#[test]
fn apple_fm_macos_run_uses_fixed_fm_respond_and_prompt_pipe_only() {
    let run = build_apple_fm_run_macos(
        "cat '/tmp/aspis prompt.d/p.txt'",
        "/usr/bin/fm",
        Some("apple-default"),
    );
    assert_eq!(
        run,
        "cat '/tmp/aspis prompt.d/p.txt' | '/usr/bin/fm' respond --model 'apple-default'"
    );
    assert!(!run.contains("TOP_SECRET_PROMPT"));
}

#[test]
fn omlx_macos_trap_cleans_key_dir_when_keyed() {
    fn q(v: &str) -> String {
        format!("'{}'", v.replace('\'', "'\\''"))
    }
    let prompt_dir = q("/tmp/aspis-agent-prompt-abc.d");
    let raw = q("/tmp/scratch/d1.json.raw");
    let key_dir = q("/tmp/aspis-agent-prompt-key.d");
    // Keyed but not sandboxed here (this test isolates key-dir handling): profile dir
    // None, sandboxed false.
    let preamble = build_macos_trap_preamble(&prompt_dir, &raw, Some(&key_dir), None, false);
    // The key dir is assigned and removed by the trap (double-quoted var) on EXIT.
    assert!(
        preamble.contains(&format!("_MINI_KEY_DIR={key_dir}")),
        "key dir must be assigned to a var: {preamble}"
    );
    assert!(
            preamble.contains(
                "trap 'rm -rf \"$_MINI_PROMPT_DIR\" \"$_MINI_RAW_FILE\" 2>/dev/null || true; [ -n \"$_MINI_KEY_DIR\" ] && rm -rf \"$_MINI_KEY_DIR\" 2>/dev/null || true' EXIT"
            ),
            "trap must remove the key dir on every exit (guarded on non-empty): {preamble}"
        );
    // The literal key path must NOT appear inside the trap body (only the var does).
    let trap_idx = preamble.find("trap '").unwrap();
    let trap_end = preamble[trap_idx..]
        .find('\n')
        .map(|o| trap_idx + o)
        .unwrap();
    assert!(
        !preamble[trap_idx..trap_end].contains("aspis-agent-prompt-key.d"),
        "literal key path must not be embedded in the trap body"
    );
}

#[cfg(windows)]
#[test]
fn build_command_full_writes_prompt_file_off_argv() {
    // The public build_mini_command writes the prompt to a restricted temp file
    // and the prompt text NEVER appears on argv (only the file path + script do).
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let b = backend(MiniCoderBackendKind::Codex, None, None);
    let directive = p4_directive(false);
    let prompt = build_mini_prompt(&b, &directive, &root, &result_target, None);
    let build = build_mini_command(&b, &root, &result_target, &prompt, None, false).unwrap();
    let prompt_file = build.prompt_file.expect("a prompt file is created");
    let joined = argv_strings(&build.command).join(" ");
    // The full prompt body (the task text) must NOT be on argv.
    assert!(
        !joined.contains("add a docstring to foo()"),
        "prompt body leaked onto argv"
    );
    // The script references the prompt FILE, not the prompt content.
    assert!(
        joined.contains(&prompt_file.to_string_lossy().to_string()),
        "prompt file path missing"
    );
    super::super::projects::remove_restricted_temp_file(&prompt_file);
}

// BLOCKER 2 (behavioral, Windows): the REAL wrapper PowerShell, given a model
// output where prose contains `}` AND the JSON output value itself contains `{`/`}`
// and a trailing `}`, must extract the CORRECT `done` object — not be tricked by a
// first-`{`/last-`}` slice into a `failed`. Runs the generated wrapper for real.
#[cfg(windows)]
#[test]
fn windows_wrapper_balanced_walk_extracts_done_with_braces_in_output() {
    use std::process::Command;
    let scratch = std::env::temp_dir().join(format!("mc_b2win_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let result_target = scratch.join("d1.json");
    let result_path = ps_single_quote(&result_target.to_string_lossy());
    let raw_path = ps_single_quote(&format!("{}.raw", result_target.to_string_lossy()));

    // The hostile model output: leading prose with a stray `}`, then the REAL
    // result object whose `output` value embeds `foo() {bar}`, then trailing prose
    // with another `}`. first-{/last-} would over-capture and fail to parse.
    let model_line = r#"Here is the result } see below: {"status":"done","output":"fixed foo() {bar}"} done now }."#;
    // `$run` simply writes that line to stdout (Write-Output), exactly what a real
    // backend pipeline would do; the wrapper redirects it to the raw file.
    let run = format!("Write-Output {}", ps_single_quote(model_line));
    let wrapper = windows_stdout_to_result_wrapper(&run, &result_path, &raw_path);

    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &wrapper,
        ])
        .status()
        .expect("run wrapper");
    assert!(status.success(), "wrapper exited non-zero");

    let written = std::fs::read_to_string(&result_target).expect("result file");
    let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON result");
    assert_eq!(
        parsed["status"], "done",
        "balanced walk must pick the done object, got: {written}"
    );
    assert_eq!(
        parsed["output"], "fixed foo() {bar}",
        "output value must survive intact: {written}"
    );
    // The raw temp file is cleaned up by the wrapper.
    assert!(
        !result_target.with_extension("json.raw").exists()
            && !std::path::Path::new(&format!("{}.raw", result_target.to_string_lossy())).exists(),
        "raw temp file must be removed"
    );
    std::fs::remove_dir_all(&scratch).ok();
}

// BLOCKER 1 / WARNING 5 (behavioral, Windows): a MULTI-WORD api command must
// tokenize natively and actually run (here `cmd /c echo ...` proves the words are
// split into an executable + args), producing a valid `done` result via the same
// stdout->file wrapper.
#[cfg(windows)]
#[test]
fn windows_api_multiword_command_tokenizes_and_runs() {
    use std::process::Command;
    let scratch = std::env::temp_dir().join(format!("mc_apiwin_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let result_target = scratch.join("d1.json");
    let prompt_file = scratch.join("p.txt");
    std::fs::write(&prompt_file, "ignored prompt").unwrap();
    // The backend's OUTPUT is a valid result JSON; we stage it in a file the
    // multi-word command prints (braces live in the FILE, not on the command line
    // — a real api CLI command line never embeds JSON braces either).
    let json_file = scratch.join("out.json");
    std::fs::write(&json_file, r#"{"status":"done","output":"multiword ok"}"#).unwrap();

    // A real MULTI-WORD command: `cmd /c type <file>` (executable `cmd`, args
    // `/c type <path>`). If the `&`-call-operator bug were present, PowerShell
    // would try to run a single executable literally named the whole string and
    // FAIL. Native tokenization splits it correctly.
    let command = format!("cmd /c type {}", json_file.to_string_lossy());
    let b = backend(MiniCoderBackendKind::Api, None, Some(command.as_str()));
    let cmd = build_mini_command_impl(
        &b,
        &scratch,
        &result_target,
        &prompt_file,
        None,
        None,
        false,
    )
    .unwrap()
    .0;
    let script = argv_strings(&cmd).pop().unwrap();

    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .current_dir(&scratch)
        .status()
        .expect("run api script");
    assert!(status.success(), "api script exited non-zero");

    let written = std::fs::read_to_string(&result_target).expect("result file");
    let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
    assert_eq!(
        parsed["status"], "done",
        "multi-word command must run; got: {written}"
    );
    assert_eq!(parsed["output"], "multiword ok", "got: {written}");
    std::fs::remove_dir_all(&scratch).ok();
}

// oMLX-P2 (behavioral, Windows): a DOWN oMLX server (connection refused on a dead
// loopback port) makes Invoke-RestMethod throw -> the try/catch swallows it -> the
// run writes NOTHING -> the EXISTING wrapper writes the CLEAN `failed` fallback. No
// partial garbage, valid JSON, script exits 0. This proves the "non-2xx / refused
// -> clean failed" contract end-to-end (a non-2xx response also throws, same path).
#[cfg(windows)]
#[test]
fn windows_omlx_down_server_yields_clean_failed_fallback() {
    use std::process::Command;
    let scratch = std::env::temp_dir().join(format!(
        "aspis-omlx-down-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let result_target = scratch.join("d1.json");
    let prompt_file = scratch.join("p.txt");
    std::fs::write(&prompt_file, "summarize this").unwrap();

    // Port 1 on loopback: nothing listens -> immediate connection refused.
    let b = omlx_backend("any-model", "http://127.0.0.1:1");
    let cmd = build_mini_command_impl(
        &b,
        &scratch,
        &result_target,
        &prompt_file,
        None,
        None,
        false,
    )
    .unwrap()
    .0;
    let script = argv_strings(&cmd).pop().unwrap();

    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .current_dir(&scratch)
        .status()
        .expect("run omlx script");
    // The script must NOT propagate the HTTP error (try/catch + exit 0).
    assert!(
        status.success(),
        "omlx script must exit 0 even when the server is down"
    );

    let written = std::fs::read_to_string(&result_target).expect("result file written");
    let parsed: serde_json::Value =
        serde_json::from_str(&written).expect("result must be VALID JSON, not partial garbage");
    assert_eq!(
        parsed["status"], "failed",
        "a down/non-2xx oMLX server must yield the clean failed fallback; got: {written}"
    );
    // The raw capture must have been cleaned by the wrapper/finally.
    assert!(
        !scratch.join("d1.json.raw").exists(),
        "the .raw capture must be removed"
    );
    std::fs::remove_dir_all(&scratch).ok();
}

// -- macOS command-build parity (compiled + run only on macOS) -----------

#[cfg(target_os = "macos")]
fn macos_script(cmd: &portable_pty::CommandBuilder) -> String {
    // /bin/sh -c <script>: the script is the last argv entry.
    cmd.get_argv()
        .last()
        .map(|a| a.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
#[test]
fn macos_api_multiword_command_tokenizes_no_call_operator() {
    // BLOCKER 1 / WARNING 5 (macOS): the multi-word command is a pipeline target
    // for /bin/sh, which tokenizes it natively. There is no `&` call operator on
    // sh; we just assert the verbatim command rides after the stdin pipe.
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p.txt");
    let b = backend(MiniCoderBackendKind::Api, None, Some("mycli chat --json"));
    let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .unwrap()
        .0;
    let script = macos_script(&cmd);
    assert!(
        script.contains("cat '") && script.contains("| mycli chat --json"),
        "api command must tokenize natively after the piped prompt file: {script}"
    );
    // FIX 1: the prompt is delivered by piping the FILE directly (bytes
    // preserved), NOT captured into a $PROMPT var (which strips trailing
    // newlines). No `printf '%s' "$PROMPT"` and no `$(cat ...)` capture.
    assert!(
        !script.contains("\"$PROMPT\""),
        "must not deliver prompt via a $PROMPT var: {script}"
    );
    assert!(
        !script.contains("PROMPT=\"$(cat"),
        "must not capture prompt into a var: {script}"
    );
    // FIX 1 (BLOCKER-safe preamble): the trap removes the prompt dir + raw file on
    // ANY exit (success, set -e abort, missing python3). The path VARIABLES are
    // assigned BEFORE the trap (the whitespace-safe variable-indirection), so the
    // script starts with `_MINI_PROMPT_DIR=`, not the trap itself.
    assert!(
        script.starts_with("_MINI_PROMPT_DIR="),
        "preamble must assign the prompt-dir var first: {script}"
    );
    let prompt_dir_idx = script
        .find("_MINI_PROMPT_DIR=")
        .expect("prompt-dir var assigned");
    let trap_idx = script.find("trap 'rm -rf ").expect("trap cleanup present");
    assert!(
        prompt_dir_idx < trap_idx,
        "the _MINI_PROMPT_DIR assignment must precede the trap: {script}"
    );
    assert!(
        script.contains("' EXIT\n"),
        "trap must fire on EXIT: {script}"
    );
    // WARNING 7: stdout redirected to a temp file (not MINI_RAW var).
    assert!(
        script.contains("MINI_RAW_FILE"),
        "raw stdout file missing: {script}"
    );
    assert!(
        !script.contains("MINI_RAW=\"$("),
        "must not capture stdout into a var: {script}"
    );
    // BLOCKER 2: raw_decode balanced walk.
    assert!(
        script.contains("raw_decode"),
        "balanced raw_decode walk missing: {script}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_trap_cleans_prompt_dir_and_raw_on_any_exit() {
    // FIX 1 (source-content leak + newline corruption): the trap must remove BOTH
    // the restricted prompt parent dir AND the `.raw` capture, and must be the
    // very first line so it is armed before any `set -e`-abortable command.
    let scratch = std::env::temp_dir();
    let result_target = scratch.join("d1.json");
    let prompt_dir = scratch.join("mc_prompt_dir");
    let prompt_file = prompt_dir.join("p.txt");
    let b = backend(MiniCoderBackendKind::Ollama, Some("qwen2.5-coder"), None);
    // ollama (no base_url == loopback) is SANDBOXED on macOS, so a `.sb` temp is created;
    // we only inspect the script string here, so clean it up at the end.
    let (cmd, profile) = build_mini_command_impl(
        &b,
        &scratch,
        &result_target,
        &prompt_file,
        None,
        None,
        false,
    )
    .unwrap();
    let script = macos_script(&cmd);
    // The path VARIABLES are assigned first (whitespace-safe indirection), then the
    // trap references them via double-quoted `$_MINI_*` expansions on EXIT.
    assert!(
        script.starts_with("_MINI_PROMPT_DIR="),
        "preamble must assign the prompt-dir var first: {script}"
    );
    assert!(
            script.contains("trap 'rm -rf \"$_MINI_PROMPT_DIR\" \"$_MINI_RAW_FILE\" 2>/dev/null || true; [ -n \"$_MINI_KEY_DIR\" ] && rm -rf \"$_MINI_KEY_DIR\" 2>/dev/null || true; [ -n \"$_MINI_PROFILE_DIR\" ] && rm -rf \"$_MINI_PROFILE_DIR\" 2>/dev/null || true' EXIT"),
            "trap must remove prompt dir + raw + (guarded) key dir + (guarded P5) profile dir via vars on EXIT: {script}"
        );
    // Both the prompt DIR and the .raw file are assigned to the vars the trap removes.
    assert!(
        script.contains(&format!(
            "_MINI_PROMPT_DIR={}",
            sh_single_quote_local(&prompt_dir.to_string_lossy())
        )),
        "the prompt dir must be assigned to _MINI_PROMPT_DIR: {script}"
    );
    assert!(
        script.contains(".raw'\n"),
        "the .raw capture must be assigned to _MINI_RAW_FILE: {script}"
    );
    if let Some(profile) = profile {
        super::super::projects::remove_restricted_temp_file(&profile);
    }
}

// BLOCKER (FIX 1, behavioral, macOS): the prompt bytes reach the backend
// VERBATIM (trailing newline preserved), and the prompt dir + .raw are gone after
// the script exits — even when the backend writes nothing.
#[cfg(target_os = "macos")]
#[test]
fn macos_prompt_bytes_preserved_and_files_cleaned_after_exit() {
    use std::process::Command;
    let scratch = std::env::temp_dir().join(format!("mc_fix1_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let prompt_dir = scratch.join("prompt");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    let prompt_file = prompt_dir.join("p.txt");
    // A prompt WITH a trailing newline — the old $(...) capture would strip it.
    std::fs::write(&prompt_file, "line1\nline2\n").unwrap();
    let result_target = scratch.join("d1.json");
    let echoed = scratch.join("echoed.bin");
    // api backend whose "command" tees stdin to a file then emits a valid result.
    let command = format!(
        "tee {} >/dev/null; printf '%s' '{{\"status\":\"done\",\"output\":\"ok\"}}'",
        echoed.to_string_lossy()
    );
    let b = backend(MiniCoderBackendKind::Api, None, Some(command.as_str()));
    let cmd = build_mini_command_impl(
        &b,
        &scratch,
        &result_target,
        &prompt_file,
        None,
        None,
        false,
    )
    .unwrap()
    .0;
    let script = macos_script(&cmd);
    let status = Command::new("/bin/sh")
        .args(["-c", &script])
        .status()
        .expect("run script");
    assert!(status.success(), "script exited non-zero");
    // Prompt bytes preserved EXACTLY (trailing newline kept).
    let seen = std::fs::read(&echoed).expect("echoed prompt");
    assert_eq!(
        seen, b"line1\nline2\n",
        "prompt bytes must be delivered verbatim"
    );
    // The restricted prompt dir + .raw capture are GONE (trap fired on EXIT).
    assert!(
        !prompt_dir.exists(),
        "prompt dir must be removed by the trap"
    );
    let raw = scratch.join("d1.json.raw");
    assert!(!raw.exists(), "raw capture must be removed by the trap");
    std::fs::remove_dir_all(&scratch).ok();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_codex_adds_mcp_flags_only_with_roots_p3() {
    // MINOR 9 → P3 (macOS): WITH roots the codex mini now carries the shared
    // `-c mcp_servers.*` tokens (read-only scope enforced SERVER-side by the
    // "mini" role); WITHOUT roots the command stays byte-identical to the
    // old no-grant status quo.
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p.txt");
    let b = backend(MiniCoderBackendKind::Codex, Some("gpt-5-codex"), None);
    let roots = McpRoots {
        management_root: PathBuf::from("/mgmt"),
        projects_dir: PathBuf::from("/mgmt/projects"),
    };
    let cmd = build_mini_command_impl(
        &b,
        &root,
        &result_target,
        &prompt_file,
        None,
        Some(&roots),
        false,
    )
    .unwrap()
    .0;
    let script = macos_script(&cmd);
    // Every arg is single-quoted by sh_single_quote_local (semantically
    // identical for /bin/sh: 'exec' is still the literal word exec).
    assert!(
        script.contains("| codex 'exec'"),
        "codex exec missing: {script}"
    );
    assert!(
        script.contains("'-m' 'gpt-5-codex'"),
        "model flag missing: {script}"
    );
    assert!(
        script.contains("mcp_servers.aspis-management.command"),
        "granted mini must carry the MCP server flags: {script}"
    );

    let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
        .unwrap()
        .0;
    let script = macos_script(&cmd);
    assert!(
        !script.contains("mcp_servers"),
        "ungranted mini must never get MCP: {script}"
    );
    assert!(
        !script.contains("'-c'"),
        "ungranted mini must never get a -c flag: {script}"
    );
}

// BLOCKER 2 (behavioral, macOS): run the REAL python wrapper to prove the
// balanced raw_decode walk extracts the correct done object despite trailing `}`.
#[cfg(target_os = "macos")]
#[test]
fn macos_wrapper_balanced_walk_extracts_done_with_braces_in_output() {
    use std::process::Command;
    let scratch = std::env::temp_dir().join(format!("mc_b2mac_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let result_target = scratch.join("d1.json");
    let result_path = sh_single_quote_local(&result_target.to_string_lossy());
    let raw_path = sh_single_quote_local(&format!("{}.raw", result_target.to_string_lossy()));

    let model_line = r#"Here is the result } see below: {"status":"done","output":"fixed foo() {bar}"} done now }."#;
    let run = format!("printf '%s' {}", sh_single_quote_local(model_line));
    let wrapper = macos_stdout_to_result_wrapper(&run, &result_path, &raw_path);

    let status = Command::new("/bin/sh")
        .args(["-c", &wrapper])
        .status()
        .expect("run wrapper");
    assert!(status.success(), "wrapper exited non-zero");

    let written = std::fs::read_to_string(&result_target).expect("result file");
    let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
    assert_eq!(parsed["status"], "done", "got: {written}");
    assert_eq!(parsed["output"], "fixed foo() {bar}", "got: {written}");
    std::fs::remove_dir_all(&scratch).ok();
}

// FIX2 (behavioral, macOS): the oMLX finish_reason=='length' truncation emitter writes
// a DISTINCT `{"status":"failed","output":"generation truncated at max_tokens ..."}` to
// stdout. The REAL python extractor must surface that message VERBATIM (not replace it
// with the generic "no valid JSON result" fallback) so truncation is observable to the
// parent coder.
#[cfg(target_os = "macos")]
#[test]
fn macos_wrapper_surfaces_truncation_failed_message_verbatim() {
    use std::process::Command;
    let scratch = std::env::temp_dir().join(format!("mc_fix2trunc_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let result_target = scratch.join("t1.json");
    let result_path = sh_single_quote_local(&result_target.to_string_lossy());
    let raw_path = sh_single_quote_local(&format!("{}.raw", result_target.to_string_lossy()));

    // Exactly what the truncation arm emits (see build oMLX wrapper).
    let model_line = r#"{"status":"failed","output":"generation truncated at max_tokens (4096) — increase budget or reduce scope"}"#;
    let run = format!("printf '%s' {}", sh_single_quote_local(model_line));
    let wrapper = macos_stdout_to_result_wrapper(&run, &result_path, &raw_path);

    let status = Command::new("/bin/sh")
        .args(["-c", &wrapper])
        .status()
        .expect("run wrapper");
    assert!(status.success(), "wrapper exited non-zero");

    let written = std::fs::read_to_string(&result_target).expect("result file");
    let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
    assert_eq!(parsed["status"], "failed", "got: {written}");
    // The DISTINCT truncation message survives — NOT the generic fallback.
    let out = parsed["output"].as_str().unwrap_or_default();
    assert!(
        out.contains("generation truncated at max_tokens"),
        "truncation message swallowed; got: {written}"
    );
    assert!(
        !out.contains("no valid JSON result"),
        "must not fall through to the generic fallback: {written}"
    );
    std::fs::remove_dir_all(&scratch).ok();
}

// FIX2 regression guard (macOS): a terminal `done` object always WINS over a `failed`
// object present earlier in the same stream — surfacing `failed` must not regress the
// common case where the model self-reports failure then a wrapper appends a done.
#[cfg(target_os = "macos")]
#[test]
fn macos_wrapper_prefers_done_over_an_earlier_failed() {
    use std::process::Command;
    let scratch = std::env::temp_dir().join(format!("mc_fix2done_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let result_target = scratch.join("t2.json");
    let result_path = sh_single_quote_local(&result_target.to_string_lossy());
    let raw_path = sh_single_quote_local(&format!("{}.raw", result_target.to_string_lossy()));

    let model_line =
        r#"{"status":"failed","output":"transient"} then {"status":"done","output":"ok"}"#;
    let run = format!("printf '%s' {}", sh_single_quote_local(model_line));
    let wrapper = macos_stdout_to_result_wrapper(&run, &result_path, &raw_path);

    let status = Command::new("/bin/sh")
        .args(["-c", &wrapper])
        .status()
        .expect("run wrapper");
    assert!(status.success(), "wrapper exited non-zero");

    let written = std::fs::read_to_string(&result_target).expect("result file");
    let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
    assert_eq!(
        parsed["status"], "done",
        "terminal done must win: {written}"
    );
    assert_eq!(parsed["output"], "ok", "got: {written}");
    std::fs::remove_dir_all(&scratch).ok();
}

// ======================================================================
// P5 — Seatbelt sandbox + rlimits (macOS). Tests 1-4 exercise the PURE,
// uncfg'd profile/loopback builders (run on the Windows dev host too); 5-8
// exercise the macOS spawn arm; 9 asserts the Windows arm stays unsandboxed.
// ======================================================================

/// argv[0] (the spawned program) of a built command — for the sandbox-wrap tests.
#[cfg(target_os = "macos")]
fn macos_argv0(cmd: &portable_pty::CommandBuilder) -> String {
    cmd.get_argv()
        .first()
        .map(|a| a.to_string_lossy().to_string())
        .unwrap_or_default()
}

// P5 test 1.
#[test]
fn seatbelt_profile_version1_deny_default() {
    let root = std::env::temp_dir();
    let profile = build_seatbelt_profile(&root, &[]);
    assert!(
        profile.starts_with("(version 1)"),
        "profile must declare (version 1) first: {profile}"
    );
    assert!(
        profile.contains("(deny default)"),
        "profile must deny by default: {profile}"
    );
}

// P5 test 2.
#[test]
fn seatbelt_profile_writes_only_parameterized_paths() {
    // Use real, existing dirs so canonicalize resolves them deterministically.
    let base = std::env::temp_dir().join(format!("mc_sb2_{}", std::process::id()));
    let project_root = base.join("project");
    let scratch = base.join("scratch");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::create_dir_all(&scratch).unwrap();
    let unrelated = base.join("unrelated-not-writable");
    std::fs::create_dir_all(&unrelated).unwrap();

    let profile = build_seatbelt_profile(&project_root, &[scratch.clone()]);

    // The write section is everything between `file-write*` and the exec section.
    let write_section = profile
        .split("(allow file-write*")
        .nth(1)
        .and_then(|s| s.split("; exec:").next())
        .expect("a file-write* section exists");
    let canon_scratch = std::fs::canonicalize(&scratch).unwrap();
    assert!(
        write_section.contains(&canon_scratch.to_string_lossy().to_string()),
        "the writable path must appear under file-write*: {profile}"
    );
    // An unrelated path is NOT writable anywhere.
    let canon_unrelated = std::fs::canonicalize(&unrelated).unwrap();
    assert!(
        !profile.contains(&canon_unrelated.to_string_lossy().to_string()),
        "an unrelated path must NOT be in the profile: {profile}"
    );
    // The project root is READ-ONLY. Reads are intentionally BROAD (`(allow file-read*)`
    // with no subpath filter): a subpath-filtered file-read* makes /bin/sh SIGABRT before
    // exec because the dyld SHARED CACHE lives on a separate Preboot/Cryptexes APFS volume
    // that `(subpath "/System")` does not traverse (empirically verified vs sandbox-exec on
    // macOS 26.5.1). So the project root is readable by virtue of the broad rule; the
    // SECURITY invariant is that it is ABSENT from file-write* (emit-edits path -> Rust
    // writes the project files, the child never does).
    let canon_root = std::fs::canonicalize(&project_root).unwrap();
    let root_str = canon_root.to_string_lossy().to_string();
    assert!(
        profile.contains("(allow file-read*)"),
        "reads must be broad (a filtered file-read* aborts /bin/sh via dyld): {profile}"
    );
    assert!(
        !write_section.contains(&root_str),
        "project root must NOT be writable (emit-edits path): {profile}"
    );
    // WARNING 4: the BROAD `(subpath "/private/var/folders")` rule must NOT appear under
    // file-write* (it would grant other sessions' cache/credential dirs). Note: on a runner
    // whose $TMPDIR itself lives under /private/var/folders, the legitimate canonicalized
    // $TMPDIR subpath DOES contain that substring — so we assert on the EXACT broad rule,
    // not the substring. Reads stay broad via `(allow file-read*)`.
    assert!(
            !write_section.contains("(subpath \"/private/var/folders\")"),
            "the broad /private/var/folders rule must NOT be in file-write* (attack surface): {profile}"
        );
    // The parameterized $TMPDIR scratch is writable (same resolution the profile uses).
    let tmpdir = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let canon_tmp = std::fs::canonicalize(&tmpdir).unwrap_or(tmpdir);
    assert!(
        write_section.contains(&canon_tmp.to_string_lossy().to_string()),
        "the $TMPDIR scratch subpath must be in file-write*: {profile}"
    );
    std::fs::remove_dir_all(&base).ok();
}

// P5 test 3.
#[test]
fn seatbelt_profile_loopback_only_no_hardcoded_8000() {
    let root = std::env::temp_dir();
    let profile = build_seatbelt_profile(&root, &[]);
    // Loopback-only via valid SBPL: `remote tcp "localhost:*"` (the kernel matches both
    // 127.0.0.1 and ::1). `remote ip "…"` is NOT valid SBPL and is rejected by sandbox-exec.
    assert!(
        profile.contains("(remote tcp \"localhost:*\")"),
        "must allow loopback TCP (any port) via valid SBPL: {profile}"
    );
    assert!(
        !profile.contains("remote ip"),
        "must NOT use the invalid `remote ip` SBPL syntax (sandbox-exec rejects it): {profile}"
    );
    // PRODUCT GENERALITY: the base_url host:port is user-configurable -> NEVER a literal port.
    assert!(
        !profile.contains(":8000"),
        "the net rule must NOT hardcode :8000: {profile}"
    );
    // Net is deny-all then loopback-allow only — no blanket allow.
    assert!(
        profile.contains("(deny network*)"),
        "must deny network by default: {profile}"
    );
    assert!(
        !profile.contains("(allow network*)"),
        "must NOT blanket-allow the network: {profile}"
    );
}

// P5 test 4.
#[test]
fn seatbelt_profile_exec_allows_sh_and_python_dirs() {
    let root = std::env::temp_dir();
    let profile = build_seatbelt_profile(&root, &[]);
    assert!(
        profile.contains("(allow process-exec"),
        "must allow process-exec: {profile}"
    );
    assert!(
        profile.contains("(literal \"/bin/sh\")"),
        "must allow exec of /bin/sh: {profile}"
    );
    // The standard interpreter dirs so a PATH-resolved python3 matches on any host.
    // `/opt/homebrew` (NOT `/opt/homebrew/bin`): Seatbelt checks the SYMLINK-RESOLVED
    // real binary path (e.g. /opt/homebrew/Cellar/python@3.x/.../python3.x), so the
    // grant must cover the whole prefix or Homebrew python3 exec is denied.
    for dir in ["/usr/bin", "/bin", "/opt/homebrew", "/usr/local/bin"] {
        assert!(
            profile.contains(&format!("(subpath \"{dir}\")")),
            "must allow exec under {dir}: {profile}"
        );
    }
    // Regression: the narrow `/opt/homebrew/bin` must NOT be the exec grant (it misses
    // the resolved Cellar path).
    assert!(
        !profile.contains("(subpath \"/opt/homebrew/bin\")"),
        "exec grant must be /opt/homebrew, not the narrow /opt/homebrew/bin: {profile}"
    );
}

// P5 test 5.
#[cfg(target_os = "macos")]
#[test]
fn macos_local_backend_wraps_with_sandbox_exec() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p").join("fake-prompt.txt");
    let b = omlx_backend("qwen2.5-coder", "http://127.0.0.1:8000/v1");
    let (cmd, profile) =
        build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
            .unwrap();
    let argv: Vec<String> = cmd
        .get_argv()
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert_eq!(
        argv[0], "/usr/bin/sandbox-exec",
        "must wrap with sandbox-exec"
    );
    assert_eq!(argv[1], "-f", "must pass the profile via -f");
    assert!(
        argv[2].ends_with(".txt"),
        "argv[2] must be the .sb profile path: {argv:?}"
    );
    assert_eq!(argv[3], "/bin/sh", "the wrapped interpreter is /bin/sh");
    assert_eq!(argv[4], "-c", "the wrapped shell runs -c <script>");
    let profile = profile.expect("a profile temp must be returned for cleanup");
    // The path passed to -f is the returned profile path.
    assert_eq!(
        argv[2],
        profile.to_string_lossy(),
        "argv -f path == returned profile"
    );
    super::super::projects::remove_restricted_temp_file(&profile);
}

// P5 test 6.
#[cfg(target_os = "macos")]
#[test]
fn macos_codex_path_unchanged_no_sandbox() {
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p.txt");
    let b = backend(MiniCoderBackendKind::Codex, Some("gpt-5-codex"), None);
    let (cmd, profile) =
        build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
            .unwrap();
    assert_eq!(
        macos_argv0(&cmd),
        "/bin/sh",
        "codex must spawn /bin/sh directly (NO sandbox-exec)"
    );
    assert!(profile.is_none(), "codex must carry no .sb profile");
    let script = macos_script(&cmd);
    assert!(
        !script.contains("sandbox-exec"),
        "codex script must not reference sandbox-exec: {script}"
    );
    // The codex preamble is BYTE-FOR-BYTE-identical to the pre-P5 status quo: NO rlimit
    // lines AND NO `.sb` profile machinery at all (not even an inert empty var) — the
    // trap is exactly the pre-P5 prompt/raw/(guarded)key removal.
    assert!(
        !script.contains("ulimit -"),
        "codex must carry NO rlimit lines: {script}"
    );
    assert!(
            !script.contains("_MINI_PROFILE_DIR"),
            "non-sandboxed codex must carry NO profile-dir machinery (byte-for-byte unchanged): {script}"
        );
    assert!(
            script.contains("trap 'rm -rf \"$_MINI_PROMPT_DIR\" \"$_MINI_RAW_FILE\" 2>/dev/null || true; [ -n \"$_MINI_KEY_DIR\" ] && rm -rf \"$_MINI_KEY_DIR\" 2>/dev/null || true' EXIT"),
            "codex trap must be the exact pre-P5 string (no profile clause): {script}"
        );
}

// P5 test 7.
#[cfg(target_os = "macos")]
#[test]
fn macos_local_backend_nonloopback_url_not_sandboxed() {
    // A hand-edited oMLX config pointing OFF-box: NOT loopback -> NOT sandboxed.
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p.txt");
    let b = omlx_backend("qwen2.5-coder", "http://10.0.0.5:8000/v1");
    let (cmd, profile) =
        build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
            .unwrap();
    assert_eq!(
        macos_argv0(&cmd),
        "/bin/sh",
        "a non-loopback oMLX URL must NOT be wrapped in sandbox-exec"
    );
    assert!(
        profile.is_none(),
        "non-loopback oMLX must carry no .sb profile"
    );
    let script = macos_script(&cmd);
    assert!(
        !script.contains("ulimit -t"),
        "non-loopback path must carry NO rlimit lines: {script}"
    );
}

// P5 test 8.
#[cfg(target_os = "macos")]
#[test]
fn rlimit_preamble_order_when_sandboxed() {
    // SANDBOXED (ollama, no base_url == loopback): trap < ulimit -u < set -e, and the
    // three ulimit lines each carry `|| true`.
    let root = std::env::temp_dir();
    let result_target = root.join("d1.json");
    let prompt_file = root.join("p.txt");
    let b = backend(MiniCoderBackendKind::Ollama, Some("qwen2.5-coder"), None);
    let (cmd, profile) =
        build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
            .unwrap();
    let script = macos_script(&cmd);
    let trap_idx = script.find("trap 'rm -rf ").expect("trap present");
    let ulimit_u_idx = script.find("ulimit -u").expect("ulimit -u present");
    let set_e_idx = script.find("\nset -e\n").expect("set -e present");
    assert!(
        trap_idx < ulimit_u_idx && ulimit_u_idx < set_e_idx,
        "order must be trap < ulimit -u < set -e: {script}"
    );
    for line in ["ulimit -t", "ulimit -v", "ulimit -u"] {
        assert!(
            script.contains(&format!("{line} ")),
            "{line} must be present: {script}"
        );
    }
    // Each rejected limit must NOT abort under set -e.
    assert!(
        script.matches("2>/dev/null || true\n").count() >= 3,
        "each ulimit line must end with `2>/dev/null || true`: {script}"
    );
    // The CPU cap reuses the wall-clock cap const (no magic number drift).
    assert!(
        script.contains(&format!("ulimit -t {} ", DEFAULT_WALL_CLOCK_CAP_SECS)),
        "ulimit -t must reuse the wall-clock cap const: {script}"
    );
    if let Some(profile) = profile {
        super::super::projects::remove_restricted_temp_file(&profile);
    }

    // NON-SANDBOXED (api): ABSENT.
    let b = backend(MiniCoderBackendKind::Api, None, Some("mycli chat"));
    let (cmd, profile) =
        build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None, false)
            .unwrap();
    assert!(profile.is_none());
    let script = macos_script(&cmd);
    assert!(
        !script.contains("ulimit -"),
        "the non-sandboxed (api) path must carry NO ulimit lines: {script}"
    );
}

// P5 test 10 — REAL-PARSER validation. The string-contains tests (1-4) CANNOT catch a
// profile the macOS kernel rejects (a single invalid SBPL token aborts sandbox-exec with
// exit 65 BEFORE exec, so every local-mini launch fails closed). This test feeds the
// generated profile to the REAL /usr/bin/sandbox-exec to prove the kernel accepts it AND
// that the write/network boundary actually confines. It is GPU-free (sandbox-exec around
// `echo`/`python3 -c print(1)` is pure CPU). macOS only.
#[cfg(target_os = "macos")]
#[test]
fn seatbelt_profile_accepted_by_real_sandbox_exec() {
    use std::process::Command;

    // 1. A realistic on-disk project_root + a writable scratch dir.
    //    CRITICAL: the base MUST live OUTSIDE $TMPDIR. On this runner $TMPDIR is
    //    /private/var/folders/.../T and the profile grants WRITE to the whole $TMPDIR
    //    subpath — so a project_root under $TMPDIR would be writable via that rule and the
    //    confinement sub-check (step 5) could not distinguish read-only from writable.
    //    The crate dir (CARGO_MANIFEST_DIR) is a writable, non-$TMPDIR location during
    //    `cargo test`; we use its `target/` (git-ignored) so we never touch sources.
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("sb_real_test_{}", std::process::id()));
    let project_root = base.join("project");
    let scratch = project_root.join(MINI_SCRATCH_DIR);
    std::fs::create_dir_all(&scratch).unwrap();

    let profile = build_seatbelt_profile(&project_root, &[scratch.clone()]);

    // 2. Write the profile to a temp `.sb` file.
    let sb_path = base.join("profile.sb");
    std::fs::write(&sb_path, &profile).unwrap();
    let sb = sb_path.to_string_lossy().to_string();

    // 3. The kernel must ACCEPT the profile and let a trivial command run (catches
    //    BLOCKER 1 `process-info-pid-self` + BLOCKER 2 `remote ip` — either aborts the
    //    parser non-zero before `echo` ever runs).
    let out = Command::new("/usr/bin/sandbox-exec")
        .args(["-f", &sb, "/bin/sh", "-c", "echo ok"])
        .output()
        .expect("spawn sandbox-exec");
    assert!(
        out.status.success(),
        "sandbox-exec REJECTED the generated profile (malformed SBPL); \
             status={:?} stderr={} profile=\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        profile
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("ok"),
        "sandboxed `echo ok` produced no `ok`: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // 4. If python3 is resolvable, exec it under the sandbox (catches BLOCKER 3 — the
    //    Homebrew Cellar symlink-resolved path denial). SKIP cleanly if absent.
    let python3_present = Command::new("/bin/sh")
        .args(["-c", "command -v python3 >/dev/null 2>&1"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if python3_present {
        let py = Command::new("/usr/bin/sandbox-exec")
            .args(["-f", &sb, "/bin/sh", "-c", "python3 -c 'print(1)'"])
            .output()
            .expect("spawn sandbox-exec for python3");
        assert!(
            py.status.success() && String::from_utf8_lossy(&py.stdout).contains('1'),
            "python3 exec was DENIED under the sandbox (BLOCKER 3 — widen exec path); \
                 status={:?} stdout={} stderr={}",
            py.status,
            String::from_utf8_lossy(&py.stdout),
            String::from_utf8_lossy(&py.stderr)
        );
    }

    // 5. CONFINEMENT: project_root is READ-only (under file-read*, ABSENT from
    //    file-write*) and is NOT under /private/var/folders only if base happens to be
    //    there — but the project_root canonical path is the file-read*/write-deny
    //    boundary either way. A write to `project_root/forbidden.txt` MUST be denied and
    //    the file MUST NOT exist.
    let forbidden = project_root.join("forbidden.txt");
    let forbidden_q = forbidden.to_string_lossy().to_string();
    let conf = Command::new("/usr/bin/sandbox-exec")
        .args([
            "-f",
            &sb,
            "/bin/sh",
            "-c",
            &format!("echo x > '{forbidden_q}'"),
        ])
        .output()
        .expect("spawn sandbox-exec for confinement check");
    assert!(
        !conf.status.success(),
        "writing into the read-only project_root must be DENIED (the profile grants \
             write ONLY to $TMPDIR + scratch); status={:?} stderr={}",
        conf.status,
        String::from_utf8_lossy(&conf.stderr)
    );
    assert!(
        !forbidden.exists(),
        "the forbidden file must NOT exist after a denied sandboxed write"
    );

    // Sanity: a write INTO the granted scratch dir DOES succeed (proves the deny above
    // is the boundary, not a blanket file-write* denial).
    let allowed = scratch.join("scratch-ok.txt");
    let allowed_q = allowed.to_string_lossy().to_string();
    let ok = Command::new("/usr/bin/sandbox-exec")
        .args([
            "-f",
            &sb,
            "/bin/sh",
            "-c",
            &format!("echo x > '{allowed_q}'"),
        ])
        .output()
        .expect("spawn sandbox-exec for allowed-write check");
    assert!(
        ok.status.success() && allowed.exists(),
        "a write into the granted scratch dir must SUCCEED; status={:?} stderr={}",
        ok.status,
        String::from_utf8_lossy(&ok.stderr)
    );

    std::fs::remove_dir_all(&base).ok();
}

// ---- TRAINING RAIL: record_directive_result is called after finalize ----

/// Exercises the `record_directive_result` call-site inside `finalize_finished_mini`
/// without a full AppHandle/Tauri runtime, by calling the training-export function
/// directly with the same arguments the call-site would produce.
///
/// LOCK-ORDERING: the call is placed after `mutate_agent_live_state` returns (the
/// agent-state lock is released); we verify this structurally: the call is outside
/// the `mutate_agent_live_state` closure, after `applied.is_ok()`.
#[test]
fn finalize_training_rail_writes_directive_result_line() {
    use super::super::training_export;

    // Build a project_root / scratch_root structure:
    //   <tmp>/project_root/.aspis-mini/
    let base = std::env::temp_dir().join(format!("mc_train_rail_{}", std::process::id()));
    let project_root = base.join("project_root");
    let scratch = project_root.join(".aspis-mini");
    std::fs::create_dir_all(&scratch).unwrap();

    // Build a `done` outcome that touched src/a.rs.
    std::fs::create_dir_all(project_root.join("src")).unwrap();
    std::fs::write(project_root.join("src").join("a.rs"), b"fn a() {}").unwrap();

    let mut d = directive("train1", "coder-1");
    d.status = MiniCoderStatus::Running;
    d.task = "add a docstring".into();
    d.files = vec!["src/a.rs".into()];
    d.result_path = "train1.json".into();
    d.scratch_path = Some(scratch.to_string_lossy().to_string());

    let mut outcome = MiniCoderOutcome::default();
    outcome.status = MiniCoderStatus::Done;
    outcome.output = Some("added docstring".into());
    outcome.files_touched = vec!["src/a.rs".into()];

    // Derive project_root exactly as finalize_finished_mini does: parent of scratch.
    let derived_root = Path::new(d.scratch_path.as_deref().unwrap())
        .parent()
        .unwrap();
    assert_eq!(
        derived_root.canonicalize().ok(),
        project_root.canonicalize().ok(),
        "derived project root must equal the actual project root"
    );

    // LOCK-ORDERING CONTRACT: call with no lock held (no Mutex around this block).
    training_export::record_directive_result(derived_root, &d, &outcome);

    // Verify a `directive_result` line was written to pairs.jsonl.
    let pairs_path = project_root.join(".aspis-training").join("pairs.jsonl");
    assert!(
        pairs_path.exists(),
        ".aspis-training/pairs.jsonl must be created"
    );
    let body = std::fs::read_to_string(&pairs_path).unwrap();
    let lines: Vec<serde_json::Value> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON line"))
        .collect();
    assert_eq!(lines.len(), 1, "one directive_result line");
    let rec = &lines[0];
    assert_eq!(rec["type"], "directive_result", "type field");
    assert_eq!(rec["directiveId"], "train1", "directiveId field");
    assert_eq!(rec["parentAgentId"], "coder-1", "parentAgentId field");
    assert_eq!(rec["task"], "add a docstring", "task field");
    assert_eq!(rec["status"], "done", "status field");
    assert_eq!(
        rec["output"].as_str(),
        Some("added docstring"),
        "output field"
    );
    // filesTouched must contain src/a.rs.
    let files_touched = rec["filesTouched"].as_array().expect("filesTouched array");
    assert!(
        files_touched.iter().any(|v| v == "src/a.rs"),
        "filesTouched must contain the changed file"
    );

    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn done_sub_edits_clause_only_for_write_directives() {
    // FIX 5: "edits applied" appears ONLY when the directive actually wrote
    // (`is_write`) AND it touched files. A non-write run's touched files were
    // inspected, not edited by us — so no edits clause.
    assert_eq!(
        done_sub(2, 1, true),
        "2 files · 1 round · edits applied",
        "write directive with files -> edits clause present"
    );
    assert_eq!(
        done_sub(2, 1, false),
        "2 files · 1 round",
        "non-write directive -> NO edits clause even with touched files"
    );
    assert_eq!(
        done_sub(0, 1, true),
        "0 files · 1 round",
        "write directive with zero files -> no edits clause"
    );
    // Singular/plural still respected on both axes.
    assert_eq!(done_sub(1, 2, true), "1 file · 2 rounds · edits applied");
}

#[test]
fn clarification_banner_sub_surfaces_the_question_to_the_human() {
    // A local coder reporting needs_clarification must show its QUESTION in the
    // terminal banner (not a bare stop), so the human sees what it's blocked on.
    assert_eq!(
        clarification_banner_sub(Some("which auth provider should I wire?")),
        "needs clarification: which auth provider should I wire?",
    );
    // No / empty / whitespace question -> a plain, still-informative banner.
    assert_eq!(clarification_banner_sub(None), "needs clarification");
    assert_eq!(clarification_banner_sub(Some("   ")), "needs clarification");
}

// ---------------------------------------------------------------------------
// ROLE UNTANGLE Phase 3: app-authored (Main coder) directive scope + liveness
// ---------------------------------------------------------------------------

#[test]
fn app_authored_directive_carries_its_own_project_and_never_loses_a_parent() {
    // The UI's spawn_main_coder_directive appends parent="app-user" (an event-log
    // sentinel, never a live session) + an explicit projectId. The executor must
    // scope it from the directive itself and skip the parent-liveness auto-kill —
    // this is the hostile-review BLOCKER regression test (a session-derived scope
    // would return None and instantly fail every app-authored directive).
    fn bare_state() -> crate::backend::model::AgentLiveState {
        crate::backend::model::AgentLiveState {
            version: 2,
            updated_at: String::new(),
            sessions: Vec::new(),
            claims: Vec::new(),
            events: Vec::new(),
            rules: Vec::new(),
            state_path: String::new(),
            mcp_command: String::new(),
            mcp_client_config: String::new(),
            mini_coder_directives: Vec::new(),
            visual_check_directives: Vec::new(),
            design_request_directives: Vec::new(),
            git_push_requests: Vec::new(),
            plan_approval_requests: Vec::new(),
            consent_requests: Vec::new(),
        }
    }
    fn bare_directive(id: &str) -> MiniCoderDirective {
        MiniCoderDirective {
            id: id.into(),
            parent_agent_id: String::new(),
            status: crate::backend::mini_coder::MiniCoderStatus::Pending,
            task: "t".into(),
            files: vec!["src/a.rs".into()],
            write: true,
            write_mode: crate::backend::mini_coder::WriteMode::AgenticIterative,
            tier: Default::default(),
            project_id: None,
            backend: None,
            allow_oracle: false,
            kill_requested: false,
            steer_queue: Vec::new(),
            result_path: format!("{id}.json"),
            agent_id: None,
            created_at: "2026-07-01T00:00:00Z".into(),
            claimed_at: None,
            scratch_path: None,
            started_at: None,
            result: None,
            attempt: 0,
            parent_directive_id: None,
            pigeon_ticket: None,
        }
    }
    let state = bare_state();
    let mut d = bare_directive("main-1");
    d.parent_agent_id = "app-user".into();
    d.project_id = Some("p1".into());
    d.tier = crate::backend::mini_coder::DirectiveTier::Main;
    assert_eq!(directive_project(&state, &d).as_deref(), Some("p1"));
    assert!(!directive_parent_gone(&state, &d));

    // The sentinel WITHOUT an explicit project is NOT app-authored: no scope (the
    // claim fails cleanly) and the liveness sweep applies (parent absent -> gone).
    let mut bare = bare_directive("main-2");
    bare.parent_agent_id = "app-user".into();
    bare.project_id = None;
    assert_eq!(directive_project(&state, &bare), None);
    assert!(directive_parent_gone(&state, &bare));

    // An ordinary MCP directive still derives scope from the live parent session
    // and still obeys the sweep: absent parent -> gone, project None.
    let mut mcp = bare_directive("mini-1");
    mcp.parent_agent_id = "coder-1".into();
    assert_eq!(directive_project(&state, &mcp), None);
    assert!(directive_parent_gone(&state, &mcp));

    // Even if an MCP directive somehow carried a projectId, the sentinel gate
    // keeps it on the session-derived path (tightness: only app-user + project
    // gets the special-case).
    mcp.project_id = Some("p9".into());
    assert_eq!(directive_project(&state, &mcp), None);
    assert!(directive_parent_gone(&state, &mcp));
}
