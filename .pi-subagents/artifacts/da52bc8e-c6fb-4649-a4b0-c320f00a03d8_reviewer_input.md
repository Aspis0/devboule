# Task for reviewer

Hostile security audit of the Windows sandbox fixes on branch `windows-port` (HEAD `3c63dd1`).

Context: the previous two hostile reviews (hostile-review-full.md + hostile-review-full-2.md) found 2 CRITICAL + 9 HIGH + 12 MEDIUM issues. I have fixed 19 of them across 3 commits:

- `51b2c1c` — C-1 (is_enforced=false), C-2 (ACL backup dedup), C-4 (CREATE_SUSPENDED+ResumeThread), H-2 (JOB_OBJECT_LIMIT_PROCESS_MEMORY), H-3 (AssignProcessToJobObject error path), H-4 (Drop ordering: close Job before restore), H-5 (restored=true after restore), M-2 (CloseHandle null check), M-5 (env block double-NUL + Windows env vars), restore_path_policy compound error
- `4ec0ec4` — M-7 (WAIT_FAILED handling), H-7 (ACL recursive (OI)(CI)+/T), M-9 (dead code allow)
- `3c63dd1` — H-8 (LUA_TOKEN filtered token), H-9 (STARTUPINFOEXW + PROC_THREAD_ATTRIBUTE_HANDLE_LIST), M-4 (lpDesktop null), M-8 (NUL stdin handle), H-1 (netsh access-denied clear error), M-10 (firewall journal + cleanup_orphaned_firewall_rules at startup)

M-12 was NOT fixed (intentional — the reviewer was wrong: the test calls macos_codex_launch_line which IS macOS-specific).

YOUR TASK — audit ONLY these files for correctness + security regressions:
1. `src-tauri/src/backend/sandbox/windows.rs` (the broker: spawn_sandboxed, create_restricted_token, create_job_object, apply_net_policy, restore_net_policy, cleanup_orphaned_firewall_rules, open_null_handle, SandboxedChild, Drop)
2. `src-tauri/src/backend/sandbox/mod.rs` (is_enforced=false, dispatch)
3. `src-tauri/src/lib.rs` (cleanup_orphaned_firewall_rules call in setup)

Check these 5 things (MAXIMUM — do not go beyond 5):
1. **CREATE_SUSPENDED + ResumeThread ordering** (C-4): is the child assigned to the Job Object while suspended, THEN resumed? Verify the ResumeThread happens AFTER AssignProcessToJobObject succeeds.
2. **Drop ordering** (H-4): does Drop close the Job handle FIRST (triggering KILL_ON_JOB_CLOSE for descendants) BEFORE restoring ACLs/net? Is the null-check correct for handles that may have been taken?
3. **STARTUPINFOEXW + HANDLE_LIST** (H-9): is the attribute list initialized correctly, populated with ONLY [stdout_write, stderr_write, null_stdin], and is EXTENDED_STARTUPINFO_PRESENT set? Is DeleteProcThreadAttributeList called after CreateProcessAsUserW?
4. **SandboxGuard + error paths**: if CreateProcessAsUserW fails AFTER ACLs/net are applied, does the guard restore them? If AssignProcessToJobObject fails, are ALL handles closed (including null_stdin)?
5. **Firewall journal** (M-10): does cleanup_orphaned_firewall_rules skip journals of still-running processes (OpenProcess check)? Is there a TOCTOU or a way for it to delete a live process's rule?

Output: write your audit to `.pi-subagents/artifacts/outputs/hostile-review-fixes-3.md` with a verdict (PASS / PASS-WITH-NOTES / FAIL) and any NEW issues found. Be hostile. If you find a regression introduced by these fixes, that's a FAIL.

Read the 3 files, verify the 5 checks, write the report. Stop.

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