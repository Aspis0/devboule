# Task for reviewer

Hostile review of C2 broker + fixes. Fresh context, deepseek-v4-pro. Be FAST: verify the 5 checks below, write the output file, STOP.

**Commit**: `dfc0ddd` (fixes) + `e86e526` (original broker). Branch `windows-port`.

**Context**: The parent wrote 250 lines of unsafe Win32 broker code (CreateProcessAsUserW + restricted token + pipes + Job Object). Parent self-audited and found 4 issues. This commit (`dfc0ddd`) fixes them. Verify the fixes are correct.

**5 checks only**:

1. `HANDLE_FLAG_INHERIT` fix: `grep -n 'HANDLE_FLAG\|0x1.*HANDLE_FLAGS\|SetHandleInformation' src-tauri/src/backend/sandbox/windows.rs` — confirm the value is `0x1` (INHERIT), NOT `0x2` (PROTECT_FROM_CLOSE). Microsoft docs: HANDLE_FLAG_INHERIT = 0x00000001.

2. `AclGuard` struct: confirm it exists, has `Drop` that calls `restore_path_policy`, and `take()` disarms it. Confirm `spawn_sandboxed` wraps `apply_path_policy` result in `AclGuard::new(...)` and calls `acl_guard.take()` on success.

3. `Drop` for `SandboxedChild`: confirm it exists, kills child via `TerminateProcess` if `!self.acl_restored`, restores ACLs, closes ALL handles (including job).

4. `wait_and_restore`: confirm it sets `self.acl_restored = true` after restoring ACLs, and does NOT manually close handles (Drop does that).

5. `cargo check --tests` compiles with 0 errors: `cd src-tauri && PROTOC='C:/Users/gualt/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe' cargo check --tests --manifest-path Cargo.toml 2>&1 | tail -3`

**Output file** `reviewer/audit-broker-fixes.md`:

```markdown
## Review
### Correct (list what passed)
### Blocker (critical issues or "none")
### Note (2-3 observations even on PASS)
## Verdict
PASS / NEEDS-FIX / FAILED
```

After writing the file, your final response MUST be:
DONE — <verdict>. Output: reviewer/audit-broker-fixes.md
Do NOT run more tools. STOP.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\6b07c4ce-f5b2-4133-b7ae-008ea2287ef7\reviewer\audit-broker-fixes.md
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