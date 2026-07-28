# Task for worker

You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue. Do not continue or answer prior messages as if they are waiting for a reply. Your sole job is to execute the task below and return a focused result for that task using your tools.

Task:
Devboule C2 — Restricted Token (v1 stub with documented gap). Pure coding task. Read the plan, write the code, commit. No delegation, no oracle, no investigations.

**Working dir**: `C:\Users\gualt\Desktop\devboule`
**Branch**: `windows-port`, HEAD `bf3ce46`. Working tree CLEAN except untracked `.pi-subagents/`, `oracle/`, `advisor/` (DO NOT touch).

**Plan SSOT**: `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §Milestone C2. Read it.

**Decision (locked by parent)**: v1 ships C2 as a **documented stub**. The broker shim (CreateProcessAsUserW) is deferred to a follow-on sub-plan. This matches the plan's accepted option: "Out of v1 scope as a sub-plan if it gets too complex; document the limitation in code comments."

**Why stub is correct**:
- Windows does NOT allow token re-attachment after `CreateProcess`. `std::process::Command::spawn()` creates the process without a custom token.
- The real fix requires `CreateProcessAsUserW` which replaces `Command::spawn()` entirely on Windows — that's 200+ lines of unsafe Win32 (pipe handling, env block, exit code). Out of v1 scope.
- `is_enforced()` STAYS `false` on Windows. No false sense of security.
- C3 (filesystem ACL) is independent and CAN land with C2 as stub.

**What to implement**:

A. **Add `apply_restricted_token` to `src-tauri/src/backend/sandbox/windows.rs`**:

```rust
/// Apply a restricted token to the child process (C2).
///
/// **v1 STATUS: STUB — documented gap, NOT enforced.**
///
/// Windows does NOT allow token re-attachment after `CreateProcess`. The
/// `std::process::Command::spawn()` path creates the process without a custom
/// token, so we cannot apply `CreateRestrictedToken` post-spawn.
///
/// The real implementation requires spawning via `CreateProcessAsUserW` in a
/// thin sandbox-broker shim (writes job handle + restricted token + ACL grant
/// order). That broker is a separate sub-plan — see
/// `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §C2 decision rule.
///
/// Until the broker lands, this function is a no-op that returns `Ok(())`.
/// `is_enforced()` stays `false` on Windows, so the broker module's
/// `effective_sandbox_mode()` correctly degrades `Unattended` to `Ask`.
pub fn apply_restricted_token(_cmd: &mut std::process::Command) -> Result<(), String> {
    // TODO(C2-broker): implement CreateRestrictedToken + CreateProcessAsUserW broker.
    // See specs/PORT_MACOS_TO_WINDOWS_FINAL.md §C2. Until then, no-op.
    Ok(())
}
```

Place it AFTER `attach_to_child` and BEFORE the `#[cfg(test)] mod tests` block.

B. **Add a test** in the `tests` module of `windows.rs`:

```rust
/// C2 stub: apply_restricted_token is a no-op that returns Ok for v1.
/// When the broker lands, this test will be replaced with a real token-restriction test.
#[test]
fn apply_restricted_token_stub_returns_ok() {
    let mut cmd = std::process::Command::new("cmd.exe");
    let result = apply_restricted_token(&mut cmd);
    assert!(result.is_ok(), "v1 stub must return Ok; got {result:?}");
}
```

C. **Do NOT modify `agentic_tools.rs`**. The spawner does NOT call `apply_restricted_token` yet — the broker sub-plan will wire it in when it lands. Adding a call site now would be dead code (the stub does nothing).

D. **Do NOT modify `mod.rs`**. C2 does not change the `wrap()` or `apply_rlimits()` dispatch. `is_enforced()` STAYS `false`.

E. **Do NOT modify `Cargo.toml`**. No new features needed for the stub (the `Win32_Security` feature from M0 is enough for the real implementation later, but the stub uses none of it).

**Hard constraints**:
- async: true
- context: fork
- ONLY modify `src-tauri/src/backend/sandbox/windows.rs`
- Do NOT touch any other file
- Do NOT touch `is_enforced()`, `mod.rs`, `agentic_tools.rs`, `Cargo.toml`, `seatbelt.rs`
- Single atomic commit. NO amend.
- Commit message: `feat(sandbox): add C2 restricted token stub with documented broker gap`

**After committing**, verify:
```bash
cd 'C:\Users\gualt\Desktop\devboule'
PROTOC='C:/Users/gualt/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe' cargo check --tests --manifest-path src-tauri/Cargo.toml 2>&1 | tail -n 5
git show HEAD --stat
```

Must show: 0 errors, 1 file modified (`windows.rs` only), no other files.

**Output at the end**: print commit hash, `cargo check --tests` result, `git show HEAD --stat`. NO `acceptance-report` JSON block. Plain output. Do NOT run more tools after committing.

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