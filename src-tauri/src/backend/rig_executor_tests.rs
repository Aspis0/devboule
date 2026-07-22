//! Layer B integration tests for the directive-executor path.
//!
//! Every test is `#[ignore]` — run with:
//! ```sh
//! cargo test -p devboule rig_executor -- --ignored
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
        stuck_report: None,
        censor_summary: None,
        status: MiniCoderStatus::Pending,
        task: "test task".into(),
        files: files.iter().map(|s| s.to_string()).collect(),
        backend: None,
        write: true,
        write_mode: mini_coder::WriteMode::EmitEdits,
        tier: Default::default(),
        project_id: None,
        task_id: None,
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

// ── B5: agentic full chain vs mock LLM ────────────────────────────────

/// End-to-end agentic loop against a one-thread in-process mock OpenAI server.
///
/// The mock queues two scripted responses:
///   POST 1 → tool_calls: one `edit_file` call replacing the planted `a + b + 1` bug
///            line in `src/lib.rs` with `a + b` (arguments per the tool spec: path,
///            oldString, newString — camelCase keys).
///   POST 2 → plain content: "Fixed the off-by-one; done."
///
/// Each request body is recorded (Mutex<Vec<String>>) for assertions: exactly 2 POSTs,
/// POST 2 must carry the `role:"tool"` message whose content reflects the edit success.
///
/// run_agentic_coder signature (agentic_runner.rs:147):
///   fn run_agentic_coder(
///       base_url: String, model: String, params: SamplingParams,
///       enable_thinking: bool, system: &str, task: &str, root: PathBuf,
///       write_allowlist: Vec<String>,
///       net: crate::backend::sandbox::NetPolicy,
///       working_set: Vec<PathBuf>,
///       max_rounds: u32,
///       cancel: &std::sync::atomic::AtomicBool,
///   ) -> Result<(LoopOutcome, Vec<String>, bool, Option<String>), String>
///
/// NetPolicy (sandbox/mod.rs:12): None / Loopback / Enabled.
///   `Loopback` is the variant used by the one-shot mini calling a local oMLX server
///   on 127.0.0.1 (sandbox/seatbelt.rs:94).
/// SamplingParams (agentic_transport.rs:28): tuned() gives temp=0.6, top_p=0.95,
///   top_k=20, thinking_budget=2000, max_tokens=detected_max_tokens().
/// LoopOutcome variants (agentic_loop.rs:53): Done { output, rounds } | Aborted { reason, rounds }.
/// HttpAgentLlm::next_turn POSTs to `{base_url}/chat/completions` (agentic_transport.rs:212).
/// build_chat_request / parse_llm_turn (agentic_transport.rs:73/112) — assistant turns
///   with tool calls carry `content: null` + `tool_calls` array; tool results carry
///   `role:"tool"` + `tool_call_id`.
/// run_agent_loop (agentic_loop.rs:79): a tool error is fed back as `ERROR: {e}` and
///   the loop CONTINUES (tool_error_is_fed_back_not_fatal test in agentic_loop.rs).
/// coordinator().try_acquire_decode (oracle_coordinator.rs:48) is test-safe:
///   OnceLock-backed, cap=2 (DEFAULT_MAX_CONCURRENT_DECODES), no Tauri state.
///
/// Mock hardening (review round): the server reads each request by accumulating
/// until the header terminator, then draining EXACTLY Content-Length body bytes
/// (never the "short read = done" heuristic — not TCP semantics), logs the FULL
/// body only, serves up to 6 connections, and answers HTTP 500 "mock exhausted"
/// once the scripted queue is empty so an unexpected extra turn FAILS FAST
/// instead of hanging the 600s reqwest timeout.
fn spawn_mock_openai(
    scripted: Vec<serde_json::Value>,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let port = listener.local_addr().unwrap().port();
    // The transport appends /chat/completions (agentic_transport.rs:212).
    let base_url = format!("http://127.0.0.1:{port}");

    let requests_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let log_clone = requests_log.clone();
    let mut queue: VecDeque<serde_json::Value> = scripted.into();

    std::thread::spawn(move || {
        // Serve strictly more connections than any sane test needs (max_rounds=4
        // → at most 5 POSTs) so a runaway loop meets a fast 500, never a hang.
        for _ in 0..6 {
            let mut conn = match listener.accept() {
                Ok((c, _)) => c,
                Err(_) => break,
            };
            // Accumulate until the end of headers, then drain exactly
            // Content-Length body bytes.
            let mut acc: Vec<u8> = Vec::new();
            let mut buf = [0u8; 4096];
            let headers_end = loop {
                match conn.read(&mut buf) {
                    Ok(0) => break None,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if let Some(pos) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                            break Some(pos + 4);
                        }
                    }
                    Err(_) => break None,
                }
            };
            let Some(headers_end) = headers_end else { continue };
            let headers = String::from_utf8_lossy(&acc[..headers_end]).to_string();
            let content_length = headers
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while acc.len() < headers_end + content_length {
                match conn.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => acc.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
            // Log the COMPLETE body (pure JSON) — never a headers-only snapshot.
            let body = String::from_utf8_lossy(&acc[headers_end..]).to_string();
            log_clone.lock().unwrap().push(body);

            let response = match queue.pop_front() {
                Some(v) => {
                    let payload = v.to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                        payload.len(),
                        payload
                    )
                }
                None => {
                    let payload = "mock exhausted";
                    format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        payload.len(),
                        payload
                    )
                }
            };
            let _ = conn.write_all(response.as_bytes());
            let _ = conn.flush();
        }
    });

    (base_url, requests_log)
}

/// Extract the content of the LAST `role:"tool"` message from a logged POST body.
/// Parsing the JSON (instead of substring-matching the whole body) keeps the
/// assertions honest: the system prompt rides along in EVERY request, so words
/// like "scope" match vacuously on the raw body (review finding).
fn last_tool_message(post_body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(post_body).ok()?;
    v.get("messages")?
        .as_array()?
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .and_then(|m| m.get("content").and_then(|c| c.as_str()))
        .map(|s| s.to_string())
}

#[test]
#[ignore = "rig: agentic worker against an in-test mock OpenAI server; cargo test rig_tests -- --ignored"]
fn test_agentic_full_chain_vs_mock_llm() {
    use crate::backend::agentic_loop::LoopOutcome;
    use crate::backend::agentic_transport::SamplingParams;
    use crate::backend::sandbox::NetPolicy;

    // ── Mock server ──────────────────────────────────────────────────────
    // POST 1: one edit_file call on src/lib.rs replacing "    a + b + 1" with "    a + b".
    // NOTE: `arguments` MUST be a JSON string (OpenAI spec; parse_llm_turn reads .as_str()).
    let edit_args: String = serde_json::to_string(&serde_json::json!({
        "path": "src/lib.rs",
        "oldString": "    a + b + 1",
        "newString": "    a + b"
    }))
    .unwrap();
    let (base_url, requests_log) = spawn_mock_openai(vec![
        serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": { "name": "edit_file", "arguments": edit_args }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
        // POST 2: plain content (done).
        serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "Fixed the off-by-one; done." },
                "finish_reason": "stop"
            }]
        }),
    ]);

    // ── Temp project with planted bug ────────────────────────────────────
    let project = temp_project("b5-agentic");
    let src = project.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        &src.join("lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b + 1\n}\n",
    )
    .unwrap();

    // ── Call run_agentic_coder ───────────────────────────────────────────
    let outcome = crate::backend::agentic_runner::run_agentic_coder(
        base_url,
        "rig-model".to_string(),
        SamplingParams::tuned(),
        false,
        crate::backend::agentic_runner::AGENTIC_SYSTEM_PROMPT,
        "Fix the off-by-one bug in add() so add(2,3) == 5",
        project.clone(),
        vec!["src/lib.rs".to_string()],
        NetPolicy::Loopback,
        vec![],
        4,
        &std::sync::atomic::AtomicBool::new(false),
        None,
        None,
    );

    // ── Assert outcome ───────────────────────────────────────────────────
    match outcome {
        Ok((LoopOutcome::Done { output, .. }, touched, net_blocked, out_of_scope)) => {
            assert!(!output.is_empty(), "done output must be non-empty; got {output:?}");
            // `touched` only records on a SUCCESSFUL edit (record_touched fires after
            // the write in agentic_tools.rs edit_file) — combined with the on-disk
            // content check below this proves the edit came from the tool execution.
            assert!(
                touched.contains(&"src/lib.rs".to_string()),
                "src/lib.rs must be in files_touched; got {touched:?}"
            );
            assert!(!net_blocked, "net must not be blocked (Loopback policy)");
            assert!(out_of_scope.is_none(), "no out-of-scope write expected; got {out_of_scope:?}");
        }
        Ok((other, touched, net_blocked, out_of_scope)) => {
            panic!("expected LoopOutcome::Done, got {other:?}; touched={touched:?}; net_blocked={net_blocked}; out_of_scope={out_of_scope:?}");
        }
        Err(e) => {
            panic!("run_agentic_coder returned Err: {e}");
        }
    }

    // ── Assert on-disk edit ──────────────────────────────────────────────
    let lib = std::fs::read_to_string(project.join("src/lib.rs")).expect("read src/lib.rs");
    assert!(lib.contains("    a + b\n"), "bug line must be replaced; got:\n{lib}");
    assert!(
        !lib.contains("a + b + 1"),
        "old bug line must be gone; got:\n{lib}"
    );

    // ── Assert request log ───────────────────────────────────────────────
    let log = requests_log.lock().unwrap();
    assert_eq!(log.len(), 2, "exactly 2 POSTs expected; got {}", log.len());
    // POST 2 must carry the tool result message; parse the JSON (never substring
    // the raw body — the system prompt rides along in every request).
    let tool_msg = last_tool_message(&log[1])
        .unwrap_or_else(|| panic!("POST 2 must carry a role:\"tool\" message; body:\n{}", log[1]));
    assert!(
        tool_msg.contains("edited"),
        "tool result must reflect edit success (edit_file returns 'edited <path>'); got: {tool_msg}"
    );

    // Cleanup.
    std::fs::remove_dir_all(&project).ok();
}

// ── B6: agentic allowlist blocks out-of-scope write ─────────────────────

/// The mock scripts ONE edit_file on `README.md` (NOT allowlisted — only `src/lib.rs` is)
/// then a done message. Per `run_agent_loop`: a tool error is fed back to the model
/// (as `ERROR: {e}`) and the loop CONTINUES — it does NOT abort (see
/// `tool_error_is_fed_back_not_fatal` in agentic_loop.rs). So the final outcome is
/// `LoopOutcome::Done` (the model recovers and returns a message), but the file on
/// disk must be UNCHANGED.
#[test]
#[ignore = "rig: agentic worker against an in-test mock OpenAI server; cargo test rig_tests -- --ignored"]
fn test_agentic_allowlist_blocks_out_of_scope_write() {
    use crate::backend::agentic_loop::LoopOutcome;
    use crate::backend::agentic_transport::SamplingParams;
    use crate::backend::sandbox::NetPolicy;

    // POST 1: edit_file on README.md (NOT allowlisted).
    // NOTE: `arguments` MUST be a JSON string (OpenAI spec; parse_llm_turn reads .as_str()).
    let edit_args: String = serde_json::to_string(&serde_json::json!({
        "path": "README.md",
        "oldString": "# Test",
        "newString": "# Updated"
    }))
    .unwrap();
    let (base_url, requests_log) = spawn_mock_openai(vec![
        serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": { "name": "edit_file", "arguments": edit_args }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
        // POST 2: plain content (model recovers after the error is fed back).
        serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "Done anyway." },
                "finish_reason": "stop"
            }]
        }),
    ]);

    // ── Temp project ─────────────────────────────────────────────────────
    let project = temp_project("b6-allowlist");
    let src = project.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        &src.join("lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("README.md"),
        "# Test Project\n\nThis is a test project.\n",
    )
    .unwrap();

    // ── Call run_agentic_coder (allowlist = only src/lib.rs) ─────────────
    let outcome = crate::backend::agentic_runner::run_agentic_coder(
        base_url,
        "rig-model".to_string(),
        SamplingParams::tuned(),
        false,
        crate::backend::agentic_runner::AGENTIC_SYSTEM_PROMPT,
        "Fix the off-by-one bug in add() so add(2,3) == 5",
        project.clone(),
        vec!["src/lib.rs".to_string()],
        NetPolicy::Loopback,
        vec![],
        4,
        &std::sync::atomic::AtomicBool::new(false),
        None,
        None,
    );

    // ── Outcome: Done (loop continues after a tool error; agentic_loop.rs) ─
    match outcome {
        Ok((LoopOutcome::Done { .. }, touched, _, out_of_scope)) => {
            assert!(
                !touched.contains(&"README.md".to_string()),
                "README.md must NOT be recorded as touched; got {touched:?}"
            );
            // The relative-path allowlist rejection bails via write_allowed BEFORE the
            // *_abs branches — the abs-path out_of_scope_write signal must stay unset
            // (it is only set in write_file_abs/edit_file_abs, agentic_tools.rs).
            assert!(
                out_of_scope.is_none(),
                "relative-path allowlist rejection must not set the abs-path out_of_scope signal; got {out_of_scope:?}"
            );
        }
        Ok((other, touched, net_blocked, out_of_scope)) => {
            panic!("expected LoopOutcome::Done, got {other:?}; touched={touched:?}; net_blocked={net_blocked}; out_of_scope={out_of_scope:?}");
        }
        Err(e) => {
            panic!("run_agentic_coder returned Err: {e}");
        }
    }

    // ── README.md must be UNCHANGED ──────────────────────────────────────
    let readme = std::fs::read_to_string(project.join("README.md")).expect("read README.md");
    assert!(
        readme.contains("This is a test project."),
        "README.md must be unchanged (out-of-scope write blocked); got:\n{readme}"
    );
    assert!(
        !readme.contains("# Updated"),
        "README.md must NOT contain the rejected edit; got:\n{readme}"
    );

    // ── POST 2 must carry the rejection error fed back to the model ──────
    // Parse the JSON and inspect the tool message CONTENT — substring checks on
    // the raw body are vacuous ("scope" appears in the system prompt of every
    // request; review finding).
    let log = requests_log.lock().unwrap();
    assert_eq!(log.len(), 2, "exactly 2 POSTs expected; got {}", log.len());
    let tool_msg = last_tool_message(&log[1])
        .unwrap_or_else(|| panic!("POST 2 must carry a role:\"tool\" message; body:\n{}", log[1]));
    assert!(
        tool_msg.contains("ERROR:") && tool_msg.contains("outside this task's write scope"),
        "tool result must carry the allowlist rejection (run_agent_loop feeds back \
         'ERROR: <path> is outside this task's write scope'); got: {tool_msg}"
    );

    // Cleanup.
    std::fs::remove_dir_all(&project).ok();
}

// ── B7: stuck report persists on the directive row ─────────────────────────

/// v6 Phase 5 durability cell: drive a directive to a terminal `Failed` through
/// the pure pieces that `finalize_finished_mini` chains (the finalize site, site
/// 4 of the `mini://stuck` emit) and assert the `.aspis-agents.json` directive row
/// now carries `stuckReport` with `taskId` == the directive id and `reason` ==
/// "failed".
///
/// This test exercises the FINALIZE site — `finalize_finished_mini` is the path
/// that runs for a mini that wrote a result file and was then finalized by the
/// executor. The other 3 sites (timeout reap, stuck-launching reap, parent-gone
/// reap) are bypass paths that DO NOT go through `finalize_finished_mini`.
///
/// Run with: cargo test -p devboule rig_executor -- --ignored
#[test]
#[ignore = "rig layer B: run with --ignored"]
fn test_stuck_report_persists_on_directive_row() {
    // 1) Temp projects dir (hermetic, cwd-independent).
    let projects_dir = temp_project("b7-stuck");
    let scratch = projects_dir.join(".aspis-mini");
    std::fs::create_dir_all(&scratch).unwrap();

    // 2) Seed an empty agents state file so the path-based reader resolves the
    //    directive queue (a missing file would yield `default_agent_live_state()`
    //    whose `mini_coder_directives` is empty — finalize would then not find
    //    the directive and the persist step would be a no-op).
    let agents_state = crate::backend::model::AgentLiveState {
        version: 2,
        updated_at: "2026-07-15T00:00:00Z".into(),
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
    let state_path = projects_dir.join(".aspis-agents.json");
    std::fs::write(&state_path, serde_json::to_string_pretty(&agents_state).unwrap()).unwrap();

    // 3) Build a Running directive with scratch/result set (the finalize path
    //    reads the result file under scratch_path).
    let directive_id = "d-stuck-7";
    let directive = mini_coder::MiniCoderDirective {
        id: directive_id.into(),
        parent_agent_id: "coder-1".into(),
        status: MiniCoderStatus::Running,
        task: "test task".into(),
        files: vec!["src/lib.rs".into()],
        backend: None,
        write: false,
        write_mode: mini_coder::WriteMode::EmitEdits,
        tier: Default::default(),
        project_id: None,
        task_id: None,
        allow_oracle: false,
        kill_requested: false,
        steer_queue: Vec::new(),
        result_path: "d-stuck-7.json".into(),
        agent_id: Some("mini-test-agent".into()),
        created_at: "2026-07-15T00:00:00Z".into(),
        claimed_at: Some("2026-07-15T00:00:01Z".into()),
        scratch_path: Some(scratch.to_string_lossy().to_string()),
        started_at: Some("2026-07-15T00:00:02Z".into()),
        result: None,
        stuck_report: None,
        censor_summary: None,
        attempt: 0,
        parent_directive_id: None,
        pigeon_ticket: None,
        collected: None,
    };

    // 4) Call `finalize_outcome` (the pure outcome computation `finalize_finished_mini`
    //    runs — scratch is set, but the result file is absent, so it returns
    //    `MiniCoderOutcome::failed("result file missing")`). This is the same
    //    outcome that the real finalize path stamps on the directive.
    let outcome = super::finalize_outcome(&directive);
    assert_eq!(
        outcome.status,
        MiniCoderStatus::Failed,
        "no result file -> Failed; got {:?}",
        outcome.status
    );

    // 5) Stamp the directive row with the outcome (the `mutate_agent_live_state`
    //    that `finalize_finished_mini` runs). Path-based: no AppHandle needed.
    crate::backend::agents::mutate_agent_live_state_at_path(&projects_dir, |state| {
        if let Some(d) = state
            .mini_coder_directives
            .iter_mut()
            .find(|d| d.id == directive_id)
        {
            d.status = outcome.status;
            d.result = Some(outcome.clone());
        } else {
            // The directive wasn't in the seeded state; push it so finalize's
            // mutate can stamp it (mirrors the real executor which already has
            // the directive in its snapshot).
            let mut d = directive.clone();
            d.status = outcome.status;
            d.result = Some(outcome.clone());
            state.mini_coder_directives.push(d);
        }
    })
    .unwrap();

    // 6) Build the StuckReport the finalize site would emit (same fields as
    //    `finalize_finished_mini`'s emit block) and persist it on the directive
    //    row — exactly what `persist_and_emit_stuck` does (minus the live emit,
    //    which is the fire-and-forget part we can't exercise without a real
    //    AppHandle).
    let reason = "failed"; // outcome.status == Failed
    let raw = outcome.error.as_deref().unwrap_or("");
    let report = crate::backend::stuck_report::StuckReport::new(
        directive_id,
        "coder-1",
        reason,
        directive.attempt.saturating_add(1),
        raw,
        outcome.files_touched.clone(),
        None, // no project in this hermetic dir
    );
    // Attach through the SAME production function persist_and_emit_stuck uses
    // (find predicate + clone + assignment) — not a hand-rolled re-implementation
    // (review: tautology finding). Only the app.emit half stays untested here
    // (needs an AppHandle).
    crate::backend::agents::mutate_agent_live_state_at_path(&projects_dir, |state| {
        assert!(
            super::attach_stuck_report(state, &report),
            "attach_stuck_report must find the seeded directive row"
        );
    })
    .unwrap();

    // 7) Read the persisted state and assert the directive row carries stuckReport.
    let content = std::fs::read_to_string(&state_path)
        .unwrap_or_else(|e| panic!("state file gone: {e}"));
    let state: crate::backend::model::AgentLiveState =
        serde_json::from_str(&content).unwrap_or_else(|e| panic!("invalid state JSON: {e}"));

    let row = state
        .mini_coder_directives
        .iter()
        .find(|d| d.id == directive_id)
        .unwrap_or_else(|| panic!("directive {} not in persisted state", directive_id));

    // Terminal state must be stamped.
    assert_eq!(
        row.result.as_ref().map(|r| r.status),
        Some(MiniCoderStatus::Failed),
        "finalize must stamp Failed; got {:?}",
        row.result
    );

    // stuckReport must be present with taskId == directive id and reason == "failed".
    let report = row
        .stuck_report
        .as_ref()
        .unwrap_or_else(|| panic!("stuck_report must be persisted on directive {}", directive_id));
    assert_eq!(
        report.task_id, directive_id,
        "stuckReport.taskId must equal the directive id; got {}",
        report.task_id
    );
    assert_eq!(
        report.reason, "failed",
        "stuckReport.reason must be 'failed'; got {}",
        report.reason
    );
    assert_eq!(
        report.agent_id, "coder-1",
        "stuckReport.agentId must be the parent_agent_id; got {}",
        report.agent_id
    );

    // Cleanup.
    std::fs::remove_dir_all(&projects_dir).ok();
}

// ── P5e-A fixture regen: agents-state.json ─────────────────────────────────

/// Write `rig/fixtures/agents-state.json` — a real fleet state snapshot with
/// sessions + directives. The test seeds a representative state (orchestrator
/// session, coder session, mini session, two MiniCoderDirectives — one running,
/// one failed with stuck_report — plus a directive with censor_summary),
/// reads back the persisted `.aspis-agents.json`, normalizes all timestamp-ish
/// fields to fixed literals, then pretty-writes the fixture.
///
/// Run with:
/// ```sh
/// cargo test --manifest-path src-tauri/Cargo.toml regen_agents_state_fixture -- --ignored
/// ```
#[test]
#[ignore = "P5e fixture regen: writes rig/fixtures/agents-state.json"]
fn regen_agents_state_fixture() {
    // 1) Temp projects dir (hermetic, cwd-independent).
    let projects_dir = temp_project("p5e-agents");

    // 2) Seed the agent live state.
    crate::backend::agents::mutate_agent_live_state_at_path(&projects_dir, |state| {
        // ── Sessions ──
        state.sessions = vec![
            // Orchestrator (active)
            crate::backend::model::AgentSession {
                agent_id: "orch-1".into(),
                role: "orchestrator".into(),
                model: Some("claude-sonnet-4-20250514".into()),
                status: "active".into(),
                message: Some("Planning the work…".into()),
                client: Some("claude".into()),
                current_project_id: Some("my-proj".into()),
                current_task_id: Some("T1".into()),
                current_file_path: None,
                first_seen_at: Some("2026-07-16T00:00:00Z".into()),
                last_seen_at: Some("2026-07-16T00:10:00Z".into()),
                launch_token_hash: Some("hash-orch-1".into()),
                launch_token_issued_at: Some("2026-07-16T00:00:00Z".into()),
                session_token_hash: Some("sess-hash-orch-1".into()),
                session_token_issued_at: Some("2026-07-16T00:00:01Z".into()),
                launch_consumed_at: None,
                subagents: vec![],
                needs_user: None,
                host: Some("app".into()),
                parent_agent_id: None,
                pending_question: None,
                user_reply: None,
            },
            // Main coder (active, working on a task)
            crate::backend::model::AgentSession {
                agent_id: "coder-1".into(),
                role: "coder".into(),
                model: Some("claude-sonnet-4-20250514".into()),
                status: "active".into(),
                message: Some("Implementing the scaffold…".into()),
                client: Some("claude".into()),
                current_project_id: Some("my-proj".into()),
                current_task_id: Some("T2".into()),
                current_file_path: Some("src/lib.rs".into()),
                first_seen_at: Some("2026-07-16T00:01:00Z".into()),
                last_seen_at: Some("2026-07-16T00:11:00Z".into()),
                launch_token_hash: Some("hash-coder-1".into()),
                launch_token_issued_at: Some("2026-07-16T00:01:00Z".into()),
                session_token_hash: Some("sess-hash-coder-1".into()),
                session_token_issued_at: Some("2026-07-16T00:01:01Z".into()),
                launch_consumed_at: None,
                subagents: vec![],
                needs_user: None,
                host: Some("app".into()),
                parent_agent_id: None,
                pending_question: None,
                user_reply: None,
            },
            // Mini coder (running, nested under coder-1)
            crate::backend::model::AgentSession {
                agent_id: "mini-1".into(),
                role: "mini-coder".into(),
                model: Some("gemini-2.5-flash".into()),
                status: "active".into(),
                message: Some("Running mini…".into()),
                client: Some("claude".into()),
                current_project_id: Some("my-proj".into()),
                current_task_id: Some("mini-task-1".into()),
                current_file_path: Some("src/hello.rs".into()),
                first_seen_at: Some("2026-07-16T00:02:00Z".into()),
                last_seen_at: Some("2026-07-16T00:12:00Z".into()),
                launch_token_hash: Some("hash-mini-1".into()),
                launch_token_issued_at: Some("2026-07-16T00:02:00Z".into()),
                session_token_hash: Some("sess-hash-mini-1".into()),
                session_token_issued_at: Some("2026-07-16T00:02:01Z".into()),
                launch_consumed_at: None,
                subagents: vec![],
                needs_user: None,
                host: Some("app".into()),
                parent_agent_id: Some("coder-1".into()),
                pending_question: None,
                user_reply: None,
            },
        ];

        // ── Mini-coder directives ──
        let running_directive = mini_coder::MiniCoderDirective {
            id: "d-running".into(),
            parent_agent_id: "coder-1".into(),
            stuck_report: None,
            censor_summary: None,
            status: MiniCoderStatus::Running,
            task: "Write hello function".into(),
            files: vec!["src/hello.rs".into()],
            backend: None,
            write: false,
            write_mode: mini_coder::WriteMode::EmitEdits,
            tier: Default::default(),
            project_id: Some("my-proj".into()),
            task_id: None,
            allow_oracle: false,
            kill_requested: false,
            steer_queue: vec![],
            result_path: "d-running.json".into(),
            agent_id: Some("mini-1".into()),
            created_at: "2026-07-16T00:00:00Z".into(),
            claimed_at: Some("2026-07-16T00:00:01Z".into()),
            scratch_path: None,
            started_at: Some("2026-07-16T00:02:00Z".into()),
            result: None,
            attempt: 0,
            parent_directive_id: None,
            pigeon_ticket: None,
            collected: None,
        };

        let mut failed_directive = mini_coder::MiniCoderDirective {
            id: "d-failed".into(),
            parent_agent_id: "coder-1".into(),
            stuck_report: None,
            censor_summary: None,
            status: MiniCoderStatus::Failed,
            task: "Add tests for hello".into(),
            files: vec!["tests/hello_test.rs".into()],
            backend: None,
            write: false,
            write_mode: mini_coder::WriteMode::EmitEdits,
            tier: Default::default(),
            project_id: Some("my-proj".into()),
            task_id: None,
            allow_oracle: false,
            kill_requested: false,
            steer_queue: vec![],
            result_path: "d-failed.json".into(),
            agent_id: None,
            created_at: "2026-07-16T00:00:00Z".into(),
            claimed_at: None,
            scratch_path: None,
            started_at: None,
            result: Some(mini_coder::MiniCoderOutcome {
                status: MiniCoderStatus::Failed,
                output: None,
                files_touched: vec!["tests/hello_test.rs".into()],
                edits: vec![],
                question: None,
                partial: None,
                error: Some("timeout after 3 attempts".into()),
                net_blocked: false,
                folder_write_blocked: None,
                censor_findings: None,
            }),
            attempt: 3,
            parent_directive_id: None,
            pigeon_ticket: None,
            collected: None,
        };

        // Attach a stuck_report to the failed directive via the production fn.
        let report = crate::backend::stuck_report::StuckReport::new(
            "d-failed",
            "coder-1",
            "failed",
            3,
            "error: compilation failed\n  --> tests/hello_test.rs:2:1",
            vec!["tests/hello_test.rs".into()],
            Some("my-proj".into()),
        );
        // Use the same production attach fn as persist_and_emit_stuck.
        super::attach_stuck_report(state, &report);
        failed_directive.stuck_report = Some(report);

        // Directive with censor_summary (phase-a findings).
        let censor_directive = mini_coder::MiniCoderDirective {
            id: "d-censored".into(),
            parent_agent_id: "coder-1".into(),
            stuck_report: None,
            censor_summary: Some(mini_coder::CensorMiniSummary {
                total: 2,
                files: vec!["src/auth.rs".into(), "src/db.rs".into()],
                ran: false,
            }),
            status: MiniCoderStatus::Done,
            task: "Refactor auth module".into(),
            files: vec!["src/auth.rs".into(), "src/db.rs".into()],
            backend: None,
            write: true,
            write_mode: mini_coder::WriteMode::EmitEdits,
            tier: Default::default(),
            project_id: Some("my-proj".into()),
            task_id: None,
            allow_oracle: false,
            kill_requested: false,
            steer_queue: vec![],
            result_path: "d-censored.json".into(),
            agent_id: None,
            created_at: "2026-07-16T00:00:00Z".into(),
            claimed_at: None,
            scratch_path: None,
            started_at: None,
            result: Some(mini_coder::MiniCoderOutcome {
                status: MiniCoderStatus::Done,
                output: None,
                files_touched: vec!["src/auth.rs".into(), "src/db.rs".into()],
                edits: vec![],
                question: None,
                partial: None,
                error: None,
                net_blocked: false,
                folder_write_blocked: None,
                censor_findings: None,
            }),
            attempt: 0,
            parent_directive_id: None,
            pigeon_ticket: None,
            collected: None,
        };

        state.mini_coder_directives = vec![running_directive, failed_directive, censor_directive];
    })
    .unwrap();

    // 3) Read the persisted state file.
    let state_path = projects_dir.join(".aspis-agents.json");
    let content = std::fs::read_to_string(&state_path)
        .unwrap_or_else(|e| panic!("state file missing: {e}"));
    let mut value: serde_json::Value =
        serde_json::from_str(&content).expect("invalid state JSON");

    // 4) Normalize all timestamp-ish fields to fixed literals so the fixture
    //    is diff-stable across regenerations.
    normalize_agents_state_timestamps(&mut value);

    // 5) Pretty-write to rig/fixtures/agents-state.json.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures_dir = manifest_dir
        .parent()
        .expect("CARGO_MANIFEST_DIR must have a parent (repo root)")
        .join("rig")
        .join("fixtures");
    std::fs::create_dir_all(&fixtures_dir).expect("create fixtures dir");
    let path = fixtures_dir.join("agents-state.json");
    let json = serde_json::to_string_pretty(&value).expect("value must serialize")
        + "\n";
    std::fs::write(&path, json)
        .unwrap_or_else(|e| panic!("write {path:?}: {e}"));

    // Cleanup.
    std::fs::remove_dir_all(&projects_dir).ok();
}

/// Recursively replace every timestamp-ish string value (matching an RFC3339
/// pattern like `2026-07-16T…`) with the fixed literal `"2026-07-16T00:00:00Z"`
/// so the dumped fixture is diff-stable across regenerations.
fn normalize_agents_state_timestamps(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            // Cover BOTH ISO shapes the state writers produce: "...Z" and
            // "...+00:00" (updatedAt uses the latter — leaving it unnormalized
            // made the committed fixture drift on every regen run).
            if s.starts_with("202")
                && s.contains('T')
                && (s.ends_with('Z') || s.contains("+00:00"))
            {
                *s = "2026-07-16T00:00:00Z".to_string();
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                normalize_agents_state_timestamps(v);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                normalize_agents_state_timestamps(v);
            }
        }
        _ => {}
    }
}
