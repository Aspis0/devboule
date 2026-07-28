# Task for reviewer

Hostile review of C2 commit. Fresh context, deepseek-v4-pro. Verify the diff yourself.

**Commit under review**: `dac08c7` on branch `windows-port`.
**Parent**: `bf3ce46`.
**Plan SSOT**: `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §Milestone C2.

**Expected diff**: 1 file, +29 lines:
```
src-tauri/src/backend/sandbox/windows.rs | 29 +++++++++++++++++++++++++++++
```

**What C2 does**: v1 ships as a DOCUMENTED STUB. `apply_restricted_token(cmd: &mut Command) -> Result<(), String>` returns `Ok(())` with a TODO linking to the broker sub-plan. Windows does not allow token re-attachment after `CreateProcess`, so real enforcement requires `CreateProcessAsUserW` — out of v1 scope per plan's "document the limitation" option. `is_enforced()` stays false.

**Verification (8 checks with file:line citations)**:

1. `cd 'C:\Users\gualt\Desktop\devboule' && git show dac08c7 --stat` — exactly 1 file, +29. No other files.

2. Open `src-tauri/src/backend/sandbox/windows.rs`. Confirm:
   - `pub fn apply_restricted_token(_cmd: &mut std::process::Command) -> Result<(), String>` exists
   - Returns `Ok(())` (no-op stub, NOT `todo!()` or `unimplemented!()`)
   - Doc comment explains WHY: Windows can't re-attach tokens post-spawn, broker sub-plan deferred
   - References `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §C2
   - One test: `apply_restricted_token_stub_returns_ok` asserts `result.is_ok()`

3. `is_enforced()` MUST still return `false` on Windows. Check `mod.rs` — C2 must NOT touch it.

4. `git diff bf3ce46 dac08c7 -- src-tauri/src/backend/sandbox/mod.rs` must be empty.

5. `git diff bf3ce46 dac08c7 -- src-tauri/Cargo.toml` must be empty.

6. `git diff bf3ce46 dac08c7 -- src-tauri/src/backend/agentic_tools.rs` must be empty (no call site added — correct, the broker will wire it).

7. `cargo check --tests --manifest-path src-tauri/Cargo.toml` must report 0 errors.

8. Conventional Commits: `feat(sandbox):` prefix, body explains the stub + broker gap + is_enforced stays false.

**Note section is MANDATORY**: list 2-3 Notes even on PASS:
- Is the stub honest (doesn't pretend to enforce)?
- Is the TODO clear enough for the next developer?
- Should the call site be wired now even as no-op, or wait for broker?

Return path + verdict line.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\2b31ee88-13e1-4d8d-88ec-ee005002bc5b\reviewer\audit-c2.md
This path is authoritative for this run.
Ignore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.

## Acceptance Contract
Acceptance level: attested
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Return concrete findings with file paths and severity when applicable

Required evidence: review-findings, residual-risks

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