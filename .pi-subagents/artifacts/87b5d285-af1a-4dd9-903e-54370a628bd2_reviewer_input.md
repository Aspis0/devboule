# Task for reviewer

Hostile review of C3 commit. Fresh context, deepseek-v4-pro.

**Commit**: `9249ced` on branch `windows-port`. **Parent**: `dac08c7`.

**Expected diff** (2 files, +168/-2):
```
src-tauri/Cargo.toml                     |   3 +-
src-tauri/src/backend/sandbox/windows.rs | 167 ++++++++++++++++++++++++++++++-
```

**What C3 does**: Filesystem ACL layer using `icacls` CLI. Adds `apply_path_policy` + `restore_path_policy` + `PathAclSnapshot`. Saves original ACL to temp file via `icacls /save`, adds deny-write for Everyone on readonly_root via `icacls /deny`, adds allow-write on writable_paths via `icacls /grant`, restores via `icacls /restore`. NOT wired into the spawner (broker will call it). `is_enforced()` stays false.

**Why icacls instead of raw Win32 API**: `ConvertStringSecurityDescriptorToSecurityDescriptorW` was unresolved in windows 0.58 without the `Win32_Security_Authorization` feature. The icacls approach is INCREMENTAL (adds ACEs to existing ACL, preserves existing ACEs) which is better than SDDL replace-all-DACL. Also added the `Win32_Security_Authorization` feature to Cargo.toml for the broker sub-plan that will eventually use the raw API.

**Verification (8 checks with file:line citations)**:

1. `git show 9249ced --stat` — exactly 2 files. No other files.

2. Open `windows.rs`. Confirm: `PathAclSnapshot` struct with `path` + `backup_file` fields. `apply_path_policy(policy: &SandboxPolicy) -> Result<Vec<PathAclSnapshot>, String>`. `restore_path_policy(snapshots: Vec<PathAclSnapshot>) -> Result<(), String>`. Helper functions: `canonicalize_path`, `save_acl`, `deny_write_everyone`, `allow_write_everyone`, `restore_acl`. All use `std::process::Command::new("icacls")`.

3. `is_enforced()` MUST still return false on Windows. `git diff dac08c7 9249ced -- src-tauri/src/backend/sandbox/mod.rs` must be empty.

4. `git diff dac08c7 9249ced -- src-tauri/src/backend/agentic_tools.rs` must be empty (not wired into spawner).

5. Cargo.toml: confirm ONLY `Win32_Security_Authorization` was added to the windows features list. No other Cargo.toml changes.

6. `cargo check --tests` must report 0 errors.

7. Two tests present: `apply_and_restore_path_policy_roundtrip` (readonly_root deny-write + restore) and `apply_writable_path_and_restore` (writable path allow-write + restore).

8. Conventional Commits: `feat(sandbox):` prefix, body explains icacls approach + not wired + is_enforced stays false.

**Note section is MANDATORY**: list 3 Notes even on PASS:
- Is `icacls` reliable in CI / Docker / Nano Server contexts? (might not be available)
- Does `icacls /save` + `/restore` roundtrip work correctly? (binary format, not portable)
- Is the test safe to run (does it actually set/restore ACLs on the temp file, or just test that icacls doesn't error)?
- Does the deny-write for Everyone also block the current user (who is part of Everyone)?
- Is the `Win32_Security_Authorization` feature in Cargo.toml unused for now (dead feature)?

Return path + verdict line.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\87b5d285-af1a-4dd9-903e-54370a628bd2\reviewer\audit-c3.md
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