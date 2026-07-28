# Task for reviewer

Hostile review of M0 commit on the devboule Windows port branch. You are the BUILTIN reviewer (deepseek-v4-pro, thinking max). Fresh context. Verify every claim from the diff, don't trust the worker.

**Commit under review**: `92a9ed64eca64a3c090cd03676353100d5cce2f2` on branch `windows-port`.
**Parent commit**: `7c8c56e`.
**Plan SSOT**: `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §M0.

**Expected diff**:
```
src-tauri/Cargo.toml | 2 +-
1 file changed, 1 insertion(+), 1 deletion(-)
```

The change appends 4 features to the existing `windows = "0.58"` block in `[target.'cfg(windows)'.dependencies]`:
- `Win32_System_JobObjects` (C1)
- `Win32_Security` (C2, C3)
- `Win32_System_Memory` (G)
- `Win32_NetworkManagement_WindowsFilteringPlatform` (C4)

The 9 original features must be preserved byte-identical.

**Context you should know**:
- Repo has NO root workspace Cargo.toml. 3 independent crates.
- `windows_capture` already pins `windows = "=0.61.3"` (second version, expected).
- `sysinfo v0.33.1` transitively pulls `windows v0.57.0` (third version, pre-existing, NOT caused by M0).
- Cargo.lock did NOT change.

**Verification (do all 8 with file:line citations)**:

1. `cd 'C:\Users\gualt\Desktop\devboule' && git show 92a9ed6 -- src-tauri/Cargo.toml` — confirm ONLY the windows=0.58 line changed. No other line in the file.

2. Compare the 9 pre-existing features to the parent commit's same line:
   `git show 7c8c56e:src-tauri/Cargo.toml | grep -A1 'windows = "0.58"'` vs `git show 92a9ed6:src-tauri/Cargo.toml | grep -A1 'windows = "0.58"'`
   Confirm the 9 original feature names are present in the new commit and byte-identical (no renames, no reordering, no case differences).

3. Confirm the 4 new features are EXACTLY those listed above (no typos). Compare against the docs.rs listing at https://docs.rs/crate/windows/0.58.0/features if you want to verify each name.

4. `git diff 7c8c56e 92a9ed6 --stat` — confirm the diff stat shows ONLY `src-tauri/Cargo.toml`. No `Cargo.lock`, no other files.

5. `git diff 7c8c56e 92a9ed6 -- src-tauri/Cargo.lock` — must be empty.

6. `git log --format='%H%n%an <%ae>%n%s%n%n%b' 92a9ed6^..92a9ed6` — confirm Conventional Commits format, reasonable author, complete body explaining the 4 features and the C1-C4/G mapping.

7. `ls devboule/.pi/agents/ 2>/dev/null` — should be EMPTY (project agents deleted in earlier step, don't fail if non-empty, just note).

8. Verify the 4 features are still inside `[target.'cfg(windows)'.dependencies]` block, not accidentally moved to base `[dependencies]`.

**Output format**:

```
## Review
- Correct: <evidence>
- Blocker: <issue> or "none"
- Note: <observation>

## Verdict
✅ PASS / ⚠️ NEEDS-FIX / ❌ FAILED
```

**Constraints**:
- async: true
- context: fresh
- output: `reviewer/audit-m0.md` (outputMode: file-only)
- READ-ONLY — do not edit any file
- Use read/grep/find/ls/bash (all available)
- Be specific: file:line for every finding

Return only the path + one-line verdict.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\6a3a78f5-7bcd-4719-ac76-4ee01c64fa20\reviewer\audit-m0.md
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