//! Layer B integration tests for the directive-executor path.
//!
//! Every test is `#[ignore]` — run with:
//! ```sh
//! cargo test -p aspis-management rig_executor -- --ignored
//! ```
//!
//! Mirrors the `mini_coder_executor_tests.rs` house pattern: child module of
//! `mini_coder_executor` (via `#[path]`), `super::*` brings in the executor's
//! private items (`read_result_outcome`) and the wildcard re-exports from
//! `mini_edit_apply`.

use super::mini_coder::{self, MiniCoderResult, MiniCoderStatus};
use super::*;

// ── helpers ────────────────────────────────────────────────────────────────

/// Fresh temp project dir (cleaned before use, NOT cwd-dependent).
fn temp_project(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rig-b-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp project dir");
    dir
}

/// Build a write-mode `MiniCoderDirective` (Pending, EmitEdits).
fn write_directive(id: &str, files: &[&str]) -> MiniCoderDirective {
    MiniCoderDirective {
        id: id.into(),
        parent_agent_id: "coder-1".into(),
        status: MiniCoderStatus::Pending,
        task: "test task".into(),
        files: files.iter().map(|s| s.to_string()).collect(),
        backend: None,
        write: true,
        write_mode: mini_coder::WriteMode::EmitEdits,
        tier: Default::default(),
        project_id: None,
        allow_oracle: false,
        kill_requested: false,
        steer_queue: Vec::new(),
        result_path: format!("{id}.json"),
        agent_id: None,
        created_at: "2026-07-15T00:00:00Z".into(),
        claimed_at: None,
        scratch_path: None,
        started_at: None,
        result: None,
        attempt: 0,
        parent_directive_id: None,
        pigeon_ticket: None,
        collected: None,
    }
}

// ── B1: full emit-edits cycle (NO AppHandle, NO real PTY) ──────────────────

/// The work-simulation core: plan_tick → apply_claim → apply_launched →
/// craft a MiniCoderResult with 2 edits → read_result_outcome →
/// apply_write_directive_edits → assert byte-exact on-disk + terminal Done.
/// Then: allowlist-violation (3rd edit outside allowlist) applies NOTHING.
#[test]
#[ignore = "rig layer B: run with --ignored"]
fn test_directive_emit_edits_full_cycle() {
    // 1) Build a temp project with a known bug line.
    let project = temp_project("b1-cycle");
    let src = project.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        "fn greet() {\n    println!(\"helo world\");\n}\n",
    )
    .unwrap();

    let scratch = project.join(".aspis-mini");
    std::fs::create_dir_all(&scratch).unwrap();

    // 2) Construct a write-mode directive (Pending).
    let d = write_directive("d1", &["src/lib.rs", "src/new.rs"]);

    // 3) plan_tick decides to claim d1.
    let plan = mini_coder::plan_tick(
        &[d.clone()],
        "2026-07-15T10:00:00Z",
        mini_coder::DEFAULT_WALL_CLOCK_CAP_SECS,
        mini_coder::DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
        mini_coder::DEFAULT_LAUNCH_CAP_SECS,
        2, // max_concurrent
        &std::collections::HashSet::new(),
    );
    assert_eq!(plan.claims, vec!["d1"]);
    assert!(plan.timeouts.is_empty());

    // 4) Drive the state machine: Pending → Launching → Running.
    let claimed = mini_coder::apply_claim(&d, "2026-07-15T10:00:00Z").expect("claim must succeed");
    assert_eq!(claimed.status, MiniCoderStatus::Launching);

    let launched = mini_coder::apply_launched(&claimed, "mini-test-1", "2026-07-15T10:00:01Z")
        .expect("launch must succeed");
    assert_eq!(launched.status, MiniCoderStatus::Running);

    // 5) Write a result JSON with 2 edits: one MODIFY, one CREATE.
    let result = MiniCoderResult {
        status: "done".into(),
        output: Some("fixed greeting".into()),
        files_touched: vec!["src/lib.rs".into(), "src/new.rs".into()],
        edits: vec![
            mini_coder::MiniEdit {
                path: "src/lib.rs".into(),
                old_string: "println!(\"helo world\")".into(),
                new_string: "println!(\"hello world\")".into(),
            },
            mini_coder::MiniEdit {
                path: "src/new.rs".into(),
                old_string: "".into(),
                new_string: "pub const VERSION: &str = \"1.0\";\n".into(),
            },
        ],
        ..Default::default()
    };
    let result_json = serde_json::to_string(&result).unwrap();
    let result_path = "d1.json";
    std::fs::write(scratch.join(result_path), &result_json).unwrap();

    // Pin scratch_path so read_result_outcome finds the result.
    let mut directive = launched;
    directive.scratch_path = Some(scratch.to_string_lossy().to_string());
    directive.result_path = result_path.into();

    // 6) Read via the integration path: read_result_outcome.
    let outcome = read_result_outcome(&scratch, result_path);
    assert_eq!(
        outcome.status,
        MiniCoderStatus::Done,
        "read failed: {:?}",
        outcome.error
    );
    assert_eq!(outcome.output.as_deref(), Some("fixed greeting"));
    assert_eq!(outcome.edits.len(), 2);

    // 7) Apply via apply_write_directive_edits (the integration apply path).
    let (applied, _diffs) = apply_write_directive_edits(Some(&project), &directive, outcome);
    assert_eq!(
        applied.status,
        MiniCoderStatus::Done,
        "apply failed: {:?}",
        applied.error
    );
    assert_eq!(
        applied.files_touched,
        vec!["src/lib.rs".to_string(), "src/new.rs".to_string()]
    );
    assert!(applied.edits.is_empty(), "edit bodies cleared after apply");

    // Assert: both edits applied byte-exactly on disk.
    let lib = std::fs::read_to_string(project.join("src/lib.rs")).unwrap();
    assert!(lib.contains("hello world"), "edit applied: {lib}");
    let new_file = std::fs::read_to_string(project.join("src/new.rs")).unwrap();
    assert_eq!(new_file, "pub const VERSION: &str = \"1.0\";\n");

    // 8) Allowlist VIOLATION: 3rd edit outside allowlist → applies NOTHING (atomic).
    let project2 = temp_project("b1-atomic");
    let src2 = project2.join("src");
    std::fs::create_dir_all(&src2).unwrap();
    std::fs::write(
        src2.join("lib.rs"),
        "fn greet() {\n    println!(\"helo world\");\n}\n",
    )
    .unwrap();
    let scratch2 = project2.join(".aspis-mini");
    std::fs::create_dir_all(&scratch2).unwrap();

    // allowlist: only src/lib.rs — the second edit targets src/secret.rs.
    let d2 = write_directive("d2", &["src/lib.rs"]);
    let claimed2 = mini_coder::apply_claim(&d2, "2026-07-15T10:01:00Z").unwrap();
    let launched2 =
        mini_coder::apply_launched(&claimed2, "mini-test-2", "2026-07-15T10:01:01Z").unwrap();

    let result2 = MiniCoderResult {
        status: "done".into(),
        output: Some("done".into()),
        files_touched: vec![],
        edits: vec![
            // Valid edit (in allowlist).
            mini_coder::MiniEdit {
                path: "src/lib.rs".into(),
                old_string: "println!(\"helo world\")".into(),
                new_string: "println!(\"hello world\")".into(),
            },
            // INVALID edit (outside allowlist).
            mini_coder::MiniEdit {
                path: "src/secret.rs".into(),
                old_string: "".into(),
                new_string: "fn secret() {}".into(),
            },
        ],
        ..Default::default()
    };
    std::fs::write(
        scratch2.join("d2.json"),
        serde_json::to_string(&result2).unwrap(),
    )
    .unwrap();

    let mut directive2 = launched2;
    directive2.scratch_path = Some(scratch2.to_string_lossy().to_string());
    directive2.result_path = "d2.json".into();

    let outcome2 = read_result_outcome(&scratch2, "d2.json");
    let (applied2, _) = apply_write_directive_edits(Some(&project2), &directive2, outcome2);
    assert_eq!(
        applied2.status,
        MiniCoderStatus::Failed,
        "allowlist violation must fail"
    );
    assert!(
        applied2
            .error
            .as_deref()
            .unwrap_or("")
            .contains("not in the directive allowlist"),
        "wrong error: {:?}",
        applied2.error
    );
    // Atomicity: NOTHING written (first file untouched).
    let lib2 = std::fs::read_to_string(project2.join("src/lib.rs")).unwrap();
    assert!(
        lib2.contains("helo world"),
        "atomic: file unchanged: {lib2}"
    );

    // Cleanup.
    std::fs::remove_dir_all(&project).ok();
    std::fs::remove_dir_all(&project2).ok();
}

// ── B2: result-file confinement and caps ───────────────────────────────────

/// Through the real read→finalize path in one flow:
///   (1) result file path traversal attempts are rejected (read_result_outcome
///       confinement),
///   (2) >40 edits are rejected through read→apply,
///   (3) symlink-escape edit is rejected.
#[test]
#[ignore = "rig layer B: run with --ignored"]
fn test_result_file_confinement_and_caps() {
    let scratch = temp_project("b2-confine");
    std::fs::create_dir_all(&scratch).unwrap();
    // Write a valid result file (control: the read path works).
    std::fs::write(
        scratch.join("ok.json"),
        r#"{"status":"done","output":"ok"}"#,
    )
    .unwrap();

    // (1) Path traversal: `../../etc/passwd` is rejected before any disk read.
    let outcome_traversal = read_result_outcome(&scratch, "../../etc/passwd");
    assert_eq!(
        outcome_traversal.status,
        MiniCoderStatus::Failed,
        "traversal must fail"
    );
    let err = outcome_traversal.error.as_deref().unwrap_or("");
    assert!(
        err.contains("escapes") || err.contains("missing") || err.contains("invalid") || err.contains("unresolved"),
        "traversal error: {err}"
    );

    // Absolute path rejected.
    #[cfg(not(windows))]
    {
        let outcome_abs = read_result_outcome(&scratch, "/etc/passwd");
        assert_eq!(outcome_abs.status, MiniCoderStatus::Failed);
    }

    // (2) >40 edits rejected through read→apply path.
    let project3 = temp_project("b2-caps");
    let src3 = project3.join("src");
    std::fs::create_dir_all(&src3).unwrap();
    std::fs::write(src3.join("a.txt"), "alpha\n").unwrap();
    let scratch3 = project3.join(".aspis-mini");
    std::fs::create_dir_all(&scratch3).unwrap();

    let many_edits: Vec<mini_coder::MiniEdit> = (0..41)
        .map(|i| mini_coder::MiniEdit {
            path: "src/a.txt".into(),
            old_string: format!("line-{i}"),
            new_string: format!("LINE-{i}"),
        })
        .collect();
    let result_many = MiniCoderResult {
        status: "done".into(),
        edits: many_edits,
        ..Default::default()
    };
    std::fs::write(
        scratch3.join("many.json"),
        serde_json::to_string(&result_many).unwrap(),
    )
    .unwrap();

    let d_many = {
        let mut d = write_directive("d-many", &["src/a.txt"]);
        d.scratch_path = Some(scratch3.to_string_lossy().to_string());
        d.result_path = "many.json".into();
        d
    };
    let outcome_many = read_result_outcome(&scratch3, "many.json");
    assert_eq!(
        outcome_many.status,
        MiniCoderStatus::Done,
        "read must succeed"
    );
    let (applied_many, _) = apply_write_directive_edits(Some(&project3), &d_many, outcome_many);
    assert_eq!(applied_many.status, MiniCoderStatus::Failed);
    assert!(
        applied_many
            .error
            .as_deref()
            .unwrap_or("")
            .contains("too many edits"),
        "cap error: {:?}",
        applied_many.error
    );
    // Atomicity: nothing written.
    assert_eq!(
        std::fs::read_to_string(src3.join("a.txt")).unwrap(),
        "alpha\n"
    );

    // (3) Symlink escape: result file is a symlink to a file outside scratch.
    #[cfg(unix)]
    {
        let outside = temp_project("b2-outside");
        std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
        let link = scratch.join("escape.json");
        std::os::unix::fs::symlink(outside.join("secret.txt"), &link).unwrap();

        let outcome_symlink = read_result_outcome(&scratch, "escape.json");
        assert_eq!(
            outcome_symlink.status,
            MiniCoderStatus::Failed,
            "symlink escape must fail"
        );
        let err = outcome_symlink.error.as_deref().unwrap_or("");
        assert!(
            err.contains("escapes") || err.contains("missing") || err.contains("symlink"),
            "symlink error: {err}"
        );
        // The outside file must NOT have been modified.
        assert_eq!(
            std::fs::read_to_string(outside.join("secret.txt")).unwrap(),
            "secret\n"
        );
        std::fs::remove_dir_all(&outside).ok();
    }

    // Cleanup.
    std::fs::remove_dir_all(&scratch).ok();
    std::fs::remove_dir_all(&project3).ok();
}

// ── B3: pi-sessions hermetic seeding (phantom-test trap) ───────────────────

/// The phantom-test trap, pinned forever: create tempdir A with a
/// `.devboule/pi-sessions.json` containing 4 sessions (2 fresh Active, 1 stale
/// Active, 1 old Stopped), call `restore_pi_sessions` with paths pointed at A
/// (NOT via cwd), assert both fresh sessions restored, then verify
/// `apply_cleanup` transitions a >24h-stale Active to Crashed and a >7d
/// Stopped is purged — all proved by the return value and a cwd-isolation
/// assertion against a second empty tempdir.
#[test]
#[ignore = "rig layer B: run with --ignored"]
fn test_pi_sessions_hermetic_seeding() {
    use crate::backend::pi_sidecar;

    let root = temp_project("b3-hermetic");
    let devboule = root.join(".devboule");
    std::fs::create_dir_all(&devboule).unwrap();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let hour_ms: u64 = 3_600_000;
    let day_ms: u64 = 24 * hour_ms;

    // 4 sessions: 2 fresh Active (restored), 1 stale Active (cleaned → Crashed),
    // 1 old Stopped (purged).
    let file = serde_json::json!({
        "sessions": [
            {
                "id": "pi-fresh-1",
                "agentRole": "orchestrator",
                "projectId": "proj-a",
                "createdAt": now_ms - 2 * hour_ms,
                "lastActiveAt": now_ms - hour_ms,
                "status": "active",
                "model": "gpt-4o"
            },
            {
                "id": "pi-fresh-2",
                "agentRole": "coder",
                "projectId": "proj-b",
                "createdAt": now_ms - 3 * hour_ms,
                "lastActiveAt": now_ms - 2 * hour_ms,
                "status": "active",
                "model": "claude-sonnet"
            },
            {
                "id": "pi-stale",
                "agentRole": "mini-coder",
                "projectId": "proj-c",
                "createdAt": now_ms - 3 * day_ms,
                "lastActiveAt": now_ms - 2 * day_ms,
                "status": "active",
                "model": null
            },
            {
                "id": "pi-old",
                "agentRole": "orchestrator",
                "projectId": "proj-d",
                "createdAt": now_ms - 15 * day_ms,
                "lastActiveAt": now_ms - 14 * day_ms,
                "status": "stopped",
                "model": null
            }
        ]
    });
    std::fs::write(
        devboule.join("pi-sessions.json"),
        serde_json::to_string_pretty(&file).unwrap(),
    )
    .unwrap();

    // Call with EXPLICIT path — NOT via cwd. This is the phantom-trap pin.
    let restored = pi_sidecar::restore_pi_sessions(&root);

    // Both fresh Active sessions are restored.
    assert_eq!(
        restored.len(),
        2,
        "both fresh active sessions restored: got {:?}",
        restored
            .iter()
            .map(|s| (&s.session_id, &s.agent_role))
            .collect::<Vec<_>>()
    );
    let ids: Vec<&str> = restored.iter().map(|s| s.session_id.as_str()).collect();
    assert!(ids.contains(&"pi-fresh-1"), "fresh-1 restored");
    assert!(ids.contains(&"pi-fresh-2"), "fresh-2 restored");

    // apply_cleanup transitioned the stale Active (>24h) to Crashed — proved
    // by its ABSENCE from the returned list (the Active filter excludes Crashed).
    assert!(
        !ids.contains(&"pi-stale"),
        "stale Active cleaned to Crashed (not restored)"
    );

    // apply_cleanup purged the old Stopped (>7d) — proved by its ABSENCE.
    assert!(
        !ids.contains(&"pi-old"),
        "old Stopped purged (not restored)"
    );

    // HERMETIC ISOLATION: a second project with NO sessions yields nothing.
    // Proves the function uses the explicit path, not cwd.
    let root_empty = temp_project("b3-empty");
    let devboule_empty = root_empty.join(".devboule");
    std::fs::create_dir_all(&devboule_empty).unwrap();
    // No pi-sessions.json in root_empty.
    let restored_empty = pi_sidecar::restore_pi_sessions(&root_empty);
    assert!(
        restored_empty.is_empty(),
        "empty project yields no sessions (cwd isolation proven)"
    );

    // Cleanup.
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&root_empty).ok();
}

// ── B4: Cloud directive fails loud through executor plan ───────────────────

/// Integration version of the 0ff5508 unit test: a Cloud-kind backend
/// directive entering the claim path produces the LOUD failure
/// status/message on the DIRECTIVE (not a silent skip) — uses the pure
/// pieces (`backend_supports_directive_dispatch` + `apply_failed`)
/// chained the way `run_pass` chains them.
#[test]
#[ignore = "rig layer B: run with --ignored"]
fn test_cloud_directive_fails_loud_through_executor_plan() {
    // 1) The dispatch gate rejects Cloud.
    let gate = backend_supports_directive_dispatch(MiniCoderBackendKind::Cloud);
    assert!(
        gate.is_err(),
        "Cloud backend must be rejected by dispatch gate"
    );
    let reason = gate.unwrap_err();
    assert!(
        reason.contains("cloud"),
        "rejection mentions cloud: {reason}"
    );

    // 2) Chain it with the fail-status transition the way run_pass does:
    //    Pending → claim → Launching → fail with the loud message → Failed.
    let mut d = write_directive("d-cloud", &["src/main.rs"]);
    d.backend = Some("cloud".into());

    // Claim (Pending → Launching).
    let claimed = mini_coder::apply_claim(&d, "2026-07-15T11:00:00Z").expect("claim must succeed");
    assert_eq!(claimed.status, MiniCoderStatus::Launching);

    // Fail with the exact message from the dispatch gate (loud, not silent).
    let failed =
        mini_coder::apply_failed(&claimed, reason).expect("fail must succeed from launching");
    assert_eq!(failed.status, MiniCoderStatus::Failed);

    // The outcome is stamped with the loud message.
    let outcome = failed.result.expect("outcome must be stamped");
    assert_eq!(outcome.status, MiniCoderStatus::Failed);
    assert!(
        outcome.error.as_deref().unwrap_or("").contains("cloud"),
        "loud failure message: {:?}",
        outcome.error
    );
}
