# Task for reviewer

Hostile review of Milestone C1 commit on devboule Windows port branch. Fresh context, deepseek-v4-pro. The previous reviewer of the same milestone was cut off mid-review (the output you see above is a fragment). You are starting fresh — do not rely on prior context, verify everything yourself.

**Commit under review**: `5510752` on branch `windows-port`.
**Parent commit**: `db0cb20`.
**Plan SSOT**: `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §Milestone C1.
**Spec used for implementation**: `oracle/decision-c1.md` (parent-direct spec, oracle stalled before writing it).

**Expected diff** (3 files, +178/-5):

```
src-tauri/src/backend/agentic_tools.rs   |  11 +++
src-tauri/src/backend/sandbox/mod.rs     |  21 ++++-
src-tauri/src/backend/sandbox/windows.rs | 151 +++++++++++++++++++++++++++++++
3 files changed, 178 insertions(+), 5 deletions(-)
```

**What C1 does**: First stage of the Windows sandbox stack. Creates a Windows Job Object (kill-on-close + optional ProcessMemoryLimit) before `cmd.spawn()`, stashes the HANDLE in a thread-local, then `attach_to_child(pid)` runs `OpenProcess(PROCESS_ALL_ACCESS) + AssignProcessToJobObject` right after the spawn. `is_enforced()` STAYS false on Windows (gated on C1+C2+C3+C4 + reviewer + oracle sign-off per FINAL plan line 42).

**Two-phase API (intentional deviation from plan, justified)**:
- Pre-spawn: `windows::wrap_policy()` + `windows::apply_rlimits(cmd, limits)` — creates the Job Object, stashes HANDLE in a thread-local `STASHED_JOB`.
- Post-spawn: `windows::attach_to_child(child_pid)` — pops the stashed HANDLE, opens process, assigns to job.
- The plan suggested a single `apply_sandbox_to_command(cmd, &mut Command, ...)` that does both, but that's impossible because `&mut Command` is pre-spawn and `OpenProcess`/`AssignProcessToJobObject` need the child PID (post-spawn). The two-phase split is the correct shape.

**Two spec corrections by worker (intentional, per worker's notes)**:
1. `AssignProcessToJobObject` lives in `windows::Win32::System::JobObjects`, not `Threading` as the spec wrote.
2. From inside `mod.rs`, the path to the new module is `crate::backend::sandbox::windows::`, not `super::windows::` (the spec had this wrong — `super` from `sandbox` resolves to `backend`).

**Worker confirmed `cargo check --tests` compiles with 0 errors** (last lines: "Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.76s").

**Verification (10 checks with file:line citations)**:

1. `cd 'C:\Users\gualt\Desktop\devboule' && git show 5510752 --stat` — confirm exactly 3 files. No other tracked files.

2. `is_enforced()` MUST still return false on Windows. Open `src-tauri/src/backend/sandbox/mod.rs:215-225` (or wherever `is_enforced` lives) and confirm the `#[cfg(target_os = "windows")]` arm still returns `false`. ALSO confirm the test `is_enforced_false_off_macos` is untouched.

3. Open `src-tauri/src/backend/sandbox/mod.rs` and confirm:
   - `pub mod windows;` added at the top (line 2 or so)
   - The `wrap()` function now has a `#[cfg(target_os = "windows")]` arm BEFORE the `#[cfg(not(any(target_os = "macos", target_os = "windows")))]` passthrough
   - The new Windows arm calls `crate::backend::sandbox::windows::wrap_policy(...)` (or equivalent correct path)
   - An `apply_rlimits` arm for `#[cfg(target_os = "windows")]` was added (or the existing `#[cfg(not(unix))]` no-op was replaced with a Windows-specific body calling `windows::apply_rlimits`)
   - No removal of macOS paths or breaking of `seatbelt` import

4. Open `src-tauri/src/backend/sandbox/windows.rs` (NEW, 151 lines). Confirm:
   - File starts with `#![cfg(target_os = "windows")]` or has `#[cfg(target_os = "windows")]` guards on the public functions
   - Three public functions present: `wrap_policy`, `apply_rlimits`, `attach_to_child` (with the spec'd signatures)
   - Imports `windows::Win32::Foundation::{CloseHandle, HANDLE}`, `windows::Win32::System::JobObjects::*` (including `AssignProcessToJobObject`, `CreateJobObjectW`, `SetInformationJobObject`, `JobObjectExtendedLimitInformation`, `JOBOBJECT_BASIC_LIMIT_INFORMATION`, `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), `windows::Win32::System::Threading::{OpenProcess, PROCESS_ALL_ACCESS}`
   - `wrap_policy` returns `SandboxedCommand { program, args }` (unchanged on Windows)
   - `apply_rlimits` creates a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` + `ProcessMemoryLimit` (read from `limits.addr_space_bytes`)
   - `apply_rlimits` stashes the HANDLE in a `thread_local!` static
   - `attach_to_child(pid)` pops the stashed HANDLE, calls `OpenProcess(PROCESS_ALL_ACCESS, false, pid)` then `AssignProcessToJobObject`
   - Error handling: each Win32 call uses `?` or returns `Result<(), String>`; failures log via `eprintln!` and either return early or propagate
   - The job HANDLE is intentionally NOT closed in `attach_to_child` (it must outlive the child for KILL_ON_JOB_CLOSE to apply)

5. Open `src-tauri/src/backend/agentic_tools.rs` around line 1011-1056. Confirm:
   - After `let mut child = cmd.spawn()...`, a `#[cfg(target_os = "windows")]` block was added that calls `windows::attach_to_child(child.id())` (or `pid` — match the local var name)
   - The block is ~3-5 lines and uses `if let Err(e) = ...` for graceful failure (no panic, just warn)
   - macOS/Linux paths in the same function are UNTOUCHED

6. Confirm `cargo check --tests --manifest-path src-tauri/Cargo.toml` (run with `PROTOC=<path>` env) reports 0 errors. Run:
   ```
   cd 'C:\Users\gualt\Desktop\devboule\src-tauri' && \
   PROTOC='C:/Users/gualt/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe' \
   cargo check --tests --manifest-path Cargo.toml 2>&1 | grep -E 'error\[E[0-9]+\]' | head -n 10
   ```
   Expected: empty output.

7. Confirm Cargo.toml is untouched:
   `git diff db0cb20 5510752 -- src-tauri/Cargo.toml` must be empty.
   `git diff db0cb20 5510752 -- oracle-core/Cargo.toml` must be empty.

8. Confirm `seatbelt.rs` is untouched:
   `git diff db0cb20 5510752 -- src-tauri/src/backend/sandbox/seatbelt.rs` must be empty.

9. Confirm Conventional Commits format. `git log --format='%H%n%an <%ae>%n%s%n%n%b' 5510752^..5510752`. Prefix should be `feat(sandbox):`. Body should mention the four pieces (pre-spawn job creation, post-spawn attach, kill-on-close, is_enforced stays false). Note: the author shown was "Saurias92 <gualtierimarco09@hotmail.com>" — different from the "gualt <gualt@devboule.local>" used in earlier commits. Note this in your report (not a blocker, but worth flagging).

10. **Search for any remaining TODO or stub**: `grep -n 'TODO\|unimplemented!\|todo!' src-tauri/src/backend/sandbox/windows.rs` — should return nothing or only documentation TODOs (not implementation TODOs). The plan says C1 is implementation-complete, not a stub.

**Verdict shape**:

```
## Review

### Correct (N items)
- ...

### Blocker
- none / <issue>

### Note
- (these are observations, not blockers — REQUIRED, even on PASS)

## Verdict
✅ PASS / ⚠️ NEEDS-FIX / ❌ FAILED
```

**IMPORTANT — "Note" section is mandatory on PASS**: even when you find no blockers, list at least 2-3 Notes. Examples:
- Is the thread-local `STASHED_JOB` safe under concurrent spawns? (single-thread check, multi-thread risk)
- Is the job handle leak acceptable in this codebase's lifetime? (long-lived parent, OS cleanup)
- Does `ProcessMemoryLimit` match the plan's `addr_space_bytes` semantics? (it measures "private commit" not "virtual address space")
- Are the two spec corrections (AssignProcessToJobObject path + module path) actually correct against `windows = 0.58` docs?
- Is the C2 (Restricted Token) follow-up clearly possible from this foundation?

**Constraints**:
- async: true
- context: fresh
- output: `reviewer/audit-c1.md` (outputMode: file-only)
- READ-ONLY — do NOT modify files
- Use read/grep/find/ls/bash/git
- Be specific: file:line for every finding
- ONE websearch MAX, only if a `windows = 0.58` API claim is uncertain

Return path + verdict line.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\a73cd65e-f889-4abe-ab92-fd9bfebc96e6\reviewer\audit-c1.md
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