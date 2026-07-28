# Task for worker

You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue. Do not continue or answer prior messages as if they are waiting for a reply. Your sole job is to execute the task below and return a focused result for that task using your tools.

Task:
Devboule — execute Milestone A: add `bundle.windows` block to Tauri config + smoke test. This is a pure coding task on a repo you've already touched. Do NOT do any OS-level setup.

**Context**:
- Working dir: `C:\Users\gualt\Desktop\devboule`
- Branch: `windows-port`, HEAD `68f72fb`
- Working tree clean (only `.pi-subagents/` untracked, ignore it)
- Plan SSOT: `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §Milestone A

**Step 1 — read tauri.conf.json bundle section**:

```bash
cd 'C:\Users\gualt\Desktop\devboule'
cat src-tauri/tauri.conf.json
```

Locate the `bundle` object (currently around lines 43-52). It has: `active`, `targets`, `icon`, `resources`, `externalBin`. NO `windows` key.

**Step 2 — add the `windows` block**:

Add INSIDE the `bundle` object, AFTER the `externalBin` line, a new key `windows` with this exact content (preserve existing 2-space indentation, maintain JSON validity):

```jsonc
"windows": {
  "wix": {},
  "nsis": {
    "installMode": "perMachine"
  },
  "webviewInstallMode": {
    "type": "downloadBootstrapper",
    "silent": true
  }
}
```

Add a comma after the `externalBin` line if not already there.

**Step 3 — validate the JSON**:

```bash
python -c "import json; json.load(open(r'C:\Users\gualt\Desktop\devboule\src-tauri\tauri.conf.json'))" && echo JSON_VALID
```

If invalid, STOP and report the parse error verbatim. Do NOT try to auto-fix.

**Step 4 — create the integration test file**:

Create `src-tauri/tests/tauri_conf_windows.rs` (the `tests/` directory does not exist; create it):

```rust
//! Smoke test for `bundle.windows` block in tauri.conf.json (Milestone A).
//!
//! This test does NOT need a Windows host or any platform-specific tooling —
//! it just parses the JSON config and asserts the expected shape. Run with:
//!   cargo test --manifest-path src-tauri/Cargo.toml --test tauri_conf_windows

use serde_json::Value;

#[test]
fn tauri_conf_json_has_windows_bundle_block() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read tauri.conf.json: {e}"));
    let v: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("parse tauri.conf.json: {e}"));

    assert!(v["bundle"]["active"].as_bool().unwrap_or(false),
            "bundle.active must be true");
    assert!(v["bundle"]["windows"].is_object(),
            "bundle.windows must be an object");

    if let Some(m) = v["bundle"]["windows"]["webviewInstallMode"].as_object() {
        let t = m.get("type").and_then(Value::as_str).unwrap_or("");
        assert!(
            matches!(t, "downloadBootstrapper" | "embedBootstrapper"
                          | "offlineInstaller" | "fixedRuntime" | "skip"),
            "bundle.windows.webviewInstallMode.type must be a valid Tauri value (got: {t})"
        );
        let silent = m.get("silent").and_then(Value::as_bool);
        assert_eq!(silent, Some(true),
                   "bundle.windows.webviewInstallMode.silent should be true for v1");
    } else {
        panic!("bundle.windows.webviewInstallMode must be present and an object");
    }

    let install_mode = v["bundle"]["windows"]["nsis"]["installMode"]
        .as_str().unwrap_or("");
    assert!(
        matches!(install_mode, "currentUser" | "perMachine" | "both"),
        "bundle.windows.nsis.installMode must be a valid Tauri NSISInstallerMode (got: {install_mode})"
    );

    assert_eq!(
        v["bundle"]["targets"].as_str().unwrap_or("all"), "all",
        "bundle.targets must remain 'all' to keep macOS + Windows cross-platform"
    );
}

#[test]
fn tauri_conf_json_no_unexpected_windows_keys() {
    // Gate: keeps the Windows block minimal and prevents accidental schema drift.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let windows = v["bundle"]["windows"].as_object()
        .expect("bundle.windows must be an object");
    let allowed: std::collections::HashSet<&str> =
        ["wix", "nsis", "webviewInstallMode"].iter().copied().collect();
    let extra: Vec<&str> = windows.keys()
        .filter(|k| !allowed.contains(k.as_str()))
        .map(|k| k.as_str())
        .collect();
    assert!(extra.is_empty(),
            "bundle.windows has unexpected keys: {extra:?} (v1 must stay minimal)");
}
```

Use the `edit` or `write` tool. Make sure the file is exactly as shown.

**Step 5 — verify**:

```bash
cd 'C:\Users\gualt\Desktop\devboule\src-tauri'
cargo test --test tauri_conf_windows 2>&1 | tail -n 30
```

Expected: 2 tests pass, 0 fail.

If `cargo` cannot compile (e.g. lance/lancedb still want something), report the LAST 30 lines of the error verbatim. Do NOT improvise fixes outside the tauri.conf.json + new test file.

**Step 6 — commit (if tests pass)**:

```bash
cd 'C:\Users\gualt\Desktop\devboule'
git add src-tauri/tauri.conf.json src-tauri/tests/tauri_conf_windows.rs
git diff --cached --stat
git -c user.name='gualt' -c user.email='gualt@devboule.local' commit -m "feat(bundle): add explicit bundle.windows block + schema smoke test (A)" -m "Configures the Windows installer: NSIS perMachine install, WebView2" -m "downloadBootstrapper (silent). Two integration tests guard the shape:" -m "tauri_conf_json_has_windows_bundle_block asserts the schema, and" -m "tauri_conf_json_no_unexpected_windows_keys gates against future schema drift." -m "No macOS bundle key touched; bundle.targets stays 'all'."
```

**Hard constraints**:

- async: true
- context: fork
- Do NOT modify any file other than `src-tauri/tauri.conf.json` and the new test file
- Do NOT touch `Cargo.toml`, `Cargo.lock`, any `.rs` source
- Do NOT run `cargo build` or `cargo check` (slow); only `cargo test --test tauri_conf_windows`
- Do NOT push, do NOT open a PR

**Output at the end**: print `python JSON_VALID` output, `cargo test` result (last 20 lines), `git diff --cached --stat`, and the commit hash. NO `acceptance-report` JSON block. Plain output.

Go.

## Acceptance Contract
Acceptance level: checked
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Implement the requested change without widening scope
- criterion-2: Return evidence sufficient for an independent acceptance review

Required evidence: changed-files, tests-added, commands-run, residual-risks, no-staged-files

Review gate: required by reviewer.

Finish with a fenced JSON block tagged `acceptance-report` in this shape:
Use empty arrays when no items apply; array fields contain strings unless object entries are shown.
`criteriaSatisfied[].status` must be exactly one of: satisfied, not-satisfied, not-applicable.
`commandsRun[].result` must be exactly one of: passed, failed, not-run.
`manualNotes` and `notes` are optional strings; an empty string means no note and does not satisfy `manual-notes` evidence.
```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "specific proof"
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "specific proof"
    }
  ],
  "changedFiles": [
    "src/file.ts"
  ],
  "testsAddedOrUpdated": [
    "test/file.test.ts"
  ],
  "commandsRun": [
    {
      "command": "command",
      "result": "passed",
      "summary": "short result"
    }
  ],
  "validationOutput": [
    "validation output or concise summary"
  ],
  "residualRisks": [
    "none"
  ],
  "noStagedFiles": true,
  "diffSummary": "short description of the diff",
  "reviewFindings": [
    "blocker: file.ts:12 - issue found, or no blockers"
  ],
  "manualNotes": "anything else the parent should know"
}
```