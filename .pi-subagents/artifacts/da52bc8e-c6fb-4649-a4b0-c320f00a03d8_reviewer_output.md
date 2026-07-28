## Hostile Audit Complete

**Verdict: PASS-WITH-NOTES** — all five targeted checks pass, zero security regressions.

Report written to `.pi-subagents/artifacts/outputs/hostile-review-fixes-3.md` (17KB).

### What passed cleanly
- **C-4 (CREATE_SUSPENDED + ResumeThread):** Child assigned to Job while frozen, THEN resumed. `windows.rs:875→902→915`. No escape window.
- **H-4 (Drop ordering):** Job closed first (KILL_ON_JOB_CLOSE), ACLs/net restored only after descendants die. `windows.rs:624→628`.
- **H-9 (STARTUPINFOEXW):** Attribute list with exactly `[stdout_write, stderr_write, null_stdin]`. `EXTENDED_STARTUPINFO_PRESENT` set. `DeleteProcThreadAttributeList` called immediately post-spawn. `windows.rs:835–883`.
- **SandboxGuard:** ACLs/net restored on ALL error paths via Drop. `windows.rs:493–522, 918`.
- **M-10 (firewall journal):** Correctly skips live processes. Called at app startup with proper `cfg` guard. `lib.rs:413–416`, `windows.rs:371–395`.

### New findings (5 total, none blocking)
- **MEDIUM:** `OpenProcess` handle leak in `cleanup_orphaned_firewall_rules` (`windows.rs:380`) — `.is_ok()` discards the `HANDLE` without `CloseHandle`. One leak per alive devboule PID per app startup.
- **LOW:** `stdout_read`/`stderr_read` not closed in `AssignProcessToJobObject` error path (`windows.rs:902–909`).
- **LOW:** `primary_token` not closed if `CreateRestrictedToken` fails (`windows.rs:716–732`).
- **LOW:** Intermediate error-path leaks of job/token/pipes/null_stdin between creation and spawn.
- **LOW:** `SetHandleInformation` result silently ignored in `open_null_handle` (`windows.rs:784`).

The only finding worth scheduling a fix-patch is the `OpenProcess` handle leak. The rest are error-path leaks in code paths that almost never execute.