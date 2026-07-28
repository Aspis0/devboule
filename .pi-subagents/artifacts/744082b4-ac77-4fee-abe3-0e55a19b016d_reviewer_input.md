# Task for reviewer

Hostile review of M0 commit on the devboule Windows port branch.

**Commit under review**: `92a9ed64eca64a3c090cd03676353100d5cce2f2` on branch `windows-port`.

**Diff**:
```
src-tauri/Cargo.toml | 2 +-
1 file changed, 1 insertion(+), 1 deletion(d)
```

The change appends 4 features to the existing `windows = "0.58"` block in `[target.'cfg(windows)'.dependencies]`:
- `Win32_System_JobObjects` (C1)
- `Win32_Security` (C2, C3)
- `Win32_System_Memory` (G)
- `Win32_NetworkManagement_WindowsFilteringPlatform` (C4)

Pre-existing 9 features were preserved exactly.

**Plan SSOT**: `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §M0.

**Context**:
- The repo has NO root workspace Cargo.toml — 3 independent crates: `src-tauri/`, `oracle-core/`, `devboule-mcp/`.
- `windows_capture` already pins `windows = "=0.61.3"` (second version in tree, expected).
- `sysinfo v0.33.1` transitively pulls `windows v0.57.0` (third version, pre-existing, NOT caused by this change).
- Verification: `cargo check --target x86_64-pc-windows-msvc` was attempted, `protoc` was missing on the dev box, `protoc 35.1` was installed via winget, build proceeded past windows resolution into lance/lancedb and reached `devboule` build script before failing on `binaries\devboule-mcp-x86_64-pc-windows-msvc.exe doesn't exist` (separate blocker, not M0's).
- `Cargo.lock` did NOT change after M0 (no version bumps, no dep additions — only feature flags).

**Your job**: hostile audit. Don't trust the worker. Verify each claim yourself.

**Checks (with file:line citations)**:

1. Open `src-tauri/Cargo.toml` and confirm:
   - The 9 original features are byte-identical (compare to parent commit `7c8c56e`).
   - Exactly 4 new features added, no extras.
   - No other line of the file changed.
   - The block is still inside `[target.'cfg(windows)'.dependencies]`, not base `[dependencies]`.
   - No accidental removal of `windows_capture`, `webview2-com`, or any other windows-related entry.

2. Confirm `Cargo.lock` is unchanged from parent `7c8c56e`: `git diff 7c8c56e 92a9ed6 -- src-tauri/Cargo.lock` must be empty.

3. Confirm `git diff 7c8c56e 92a9ed6` shows ONLY `src-tauri/Cargo.toml` modified. No stray file changes, no schemas/, no other Cargo.toml touched.

4. Confirm `git log --format=%s 92a9ed6^..92a9ed6` shows a single, well-formed Conventional Commits message.

5. Confirm the 4 feature names match exactly what `docs.rs/crate/windows/0.58.0/features` exposes (no typos like `Win32_System_Job_Object` or `Win32_NetworkManagementWindowsFilteringPlatform`).

6. Confirm no `.cargo/config.toml` was added (the worker should NOT add one — `cargo check` works without it on this box).

7. Confirm `specs/` is NOT in this commit (separate commit `specs` docs landed later, your review is ONLY on `92a9ed6`).

8. Check the commit author and email are reasonable (gualt / gualt@devboule.local — confirm not garbage).

**Verdict shape**:

```
### Critical (must fix)
- file.ext:line - issue

### Warnings (should fix)
- file.ext:line - issue

### Verdict
✅ PASS / ⚠️ NEEDS-FIX / ❌ FAILED
```

**Constraints**:

- async: true
- context: fresh
- output: `reviewer/audit-m0.md` (outputMode: file-only)
- read-only — do NOT modify files
- do NOT spin up further subagents; do your own verification with read/bash/git/grep
- be specific: file:line citations for every finding
- if you find a critical issue, do NOT try to fix it — just report

Return only the path + verdict line when done.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\744082b4-ac77-4fee-abe3-0e55a19b016d\reviewer\audit-m0.md
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