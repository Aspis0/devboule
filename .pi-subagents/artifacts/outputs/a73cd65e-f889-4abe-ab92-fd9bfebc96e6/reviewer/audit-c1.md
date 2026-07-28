## Review — Milestone C1 (`5510752`, `feat(sandbox): add Windows Job Object wrapper`)

### Correct (10 items)

- **C1.1 — Stat matches expected diff.** `git show 5510752 --stat` shows exactly 3 files, +178/−5: `agentic_tools.rs` (+11), `sandbox/mod.rs` (+21/−5), `sandbox/windows.rs` (+151, new). No other tracked files touched. ✅

- **C1.2 — `is_enforced()` stays false on Windows.** `mod.rs:216-222`: the `#[cfg(target_os = "windows")]` arm returns `false` with a comment noting the gate to flip when C2–C4 land. The `#[cfg(not(target_os = "macos"))]` test `is_enforced_false_off_macos` (`mod.rs:296-298`) is untouched and still compiles (Windows returns false, so `assert!(!is_enforced())` passes). ✅

- **C1.3 — `mod.rs` integration points are correct.**
  - `pub mod windows;` added at line 2, after `pub mod seatbelt;`. ✅
  - `wrap()` (`mod.rs:133-135`): `#[cfg(target_os = "windows")]` arm calls `crate::backend::sandbox::windows::wrap_policy(...)`, placed BEFORE the `#[cfg(not(any(target_os = "macos", target_os = "windows")))]` passthrough at line 137. ✅
  - `apply_rlimits` (`mod.rs:200-203`): dedicated `#[cfg(target_os = "windows")]` arm delegates to `crate::backend::sandbox::windows::apply_rlimits(cmd, limits)`. ✅
  - The `#[cfg(unix)]` `apply_rlimits` arm and the macOS `seatbelt` import are untouched. ✅

- **C1.4 — `windows.rs` structure and API match spec.**
  - File header (`windows.rs:5`): `#![cfg(target_os = "windows")]`. All public functions compiled only on Windows. ✅
  - Three public functions: `wrap_policy` (line 25), `apply_rlimits` (line 50), `attach_to_child` (line 90). ✅
  - Imports from `windows::Win32::Foundation` (`CloseHandle`, `HANDLE`), `windows::Win32::System::JobObjects` (`AssignProcessToJobObject`, `CreateJobObjectW`, `SetInformationJobObject`, `JobObjectExtendedLimitInformation`, `JOBOBJECT_BASIC_LIMIT_INFORMATION`, `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), and `windows::Win32::System::Threading` (`OpenProcess`, `PROCESS_ALL_ACCESS`). All resolvable under `windows = "0.58"`. ✅
  - `wrap_policy` (`windows.rs:25-37`): returns `SandboxedCommand { program, args }` unchanged (Windows has no argv-rewriting wrapper, unlike macOS `sandbox-exec`). ✅
  - `apply_rlimits` (`windows.rs:50-86`): creates Job Object via `CreateJobObjectW(None, None)`, configures `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` + `ProcessMemoryLimit = limits.addr_space_bytes.unwrap_or(usize::MAX)`, stashes HANDLE in `thread_local! { static STASHED_JOB … }`. ✅
  - `attach_to_child(pid)` (`windows.rs:90-112`): pops the stashed HANDLE, calls `OpenProcess(PROCESS_ALL_ACCESS, false, pid)`, then `AssignProcessToJobObject(job, proc_handle)`. ✅
  - Error handling: `eprintln!` + early `return` for `CreateJobObjectW`/`SetInformationJobObject` failures; `Result<(), String>` with `.map_err()` for `OpenProcess`/`AssignProcessToJobObject` failures. ✅
  - Job HANDLE intentionally NOT closed (`windows.rs:109-111`): `let _ = proc_handle; let _ = job;` with comment explaining KILL_ON_JOB_CLOSE requires the handle to outlive the child. ✅

- **C1.5 — `agentic_tools.rs` attach call site is correct.**
  - `agentic_tools.rs:1066-1074`: after `let mut child = cmd.spawn()…`, a `#[cfg(target_os = "windows")]` block calls `windows::attach_to_child(child.id())` with `if let Err(e) = … { eprintln!(…) }` for graceful failure (no panic). macOS/Linux paths in the same function are untouched. ✅

- **C1.6 — `cargo check --tests` passes with 0 errors.**
  - `cargo check --tests --manifest-path src-tauri/Cargo.toml` completed in 1.88s with `Finished dev profile`. 168 pre-existing warnings (snake_case naming in `roles_config.rs` tests, etc.) — no new warnings introduced by this diff. No `error[E…]` output. ✅

- **C1.7 — Cargo.toml untouched.**
  - `git diff db0cb20 5510752 -- src-tauri/Cargo.toml` → no output. ✅
  - `git diff db0cb20 5510752 -- oracle-core/Cargo.toml` → no output. ✅

- **C1.8 — `seatbelt.rs` untouched.**
  - `git diff db0cb20 5510752 -- src-tauri/src/backend/sandbox/seatbelt.rs` → no output. ✅

- **C1.9 — Conventional Commits format.**
  - Prefix: `feat(sandbox):` ✅
  - Body: "Implements the first Windows sandbox milestone: a Job Object is created before spawn (kill-on-close + optional process memory limit) and the spawned child is assigned to it right after cmd.spawn(). is_enforced() stays false on Windows until C2..C4 land, so this is defense-in-depth only." Mentions all four pieces (pre-spawn creation, post-spawn attach, kill-on-close, is_enforced stays false). ✅

- **C1.10 — No implementation TODOs or stubs.**
  - `grep -n 'TODO\|unimplemented!\|todo!' src-tauri/src/backend/sandbox/windows.rs` returns zero matches. The only TODOs in the workspace are pre-existing (Linux landlock stub in `mod.rs:141`, documented as future work). ✅

### Spec correction verification

Two intentional deviations from the parent-provided spec (`oracle/decision-c1.md`):

1. **`AssignProcessToJobObject` namespace** (spec: `windows::Win32::System::Threading`, actual: `windows::Win32::System::JobObjects`). Verified via the `windows = "0.58"` crate layout — `AssignProcessToJobObject` is exported from `windows::Win32::System::JobObjects`, not `Threading`. The worker's correction is correct. ✅

2. **Module path from `mod.rs`** (spec: `super::windows::`, actual: `crate::backend::sandbox::windows::`). From within `sandbox/mod.rs`, `super` resolves to `backend`. `super::windows` would be `backend::windows` — which does not exist. The worker's path `crate::backend::sandbox::windows` is correct and explicit. ✅

### Blocker

None.

### Notes

- **N1 — Thread-local stash is single-slot per thread.** `STASHED_JOB` is a `thread_local!` with capacity for one HANDLE. If a single thread calls `apply_rlimits` twice without an intervening `attach_to_child` (e.g., spawn fails, then another spawn succeeds), the second `apply_rlimits` overwrites the stash and the first job handle is silently leaked. In the current codebase, the only call site (`agentic_tools.rs:1063-1074`) calls `apply_rlimits` immediately before `cmd.spawn()` and `attach_to_child` immediately after — the sequence is atomic within the `run()` function. A `cmd.spawn()` failure short-circuits via `?`, but the leaked handle is harmless (OS cleans up at parent exit, same intentional leak pattern as the success path). Not a bug today, but fragile if spawning is ever parallelized or the call sequence is reordered.

- **N2 — Memory-limit semantics differ from Unix `RLIMIT_AS`.** `JOBOBJECT_EXTENDED_LIMIT_INFORMATION.ProcessMemoryLimit` enforces "private commit charge" (committed virtual memory that cannot be shared), not total virtual address space size (`RLIMIT_AS` on Unix). For a runaway-task guard, this is sufficient — an infinite allocator loop will still hit the cap. However, `max_procs` from `ResourceLimits` is silently ignored on Windows (the Unix arm sets `RLIMIT_NPROC`, but the Windows arm does not set `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`). Both differences are acknowledged in the spec (`decision-c1.md` §"Risks + open questions," items 4 and 5), so this is not a defect — just a semantic gap worth documenting in `ResourceLimits` doc comments when C4 lands.

- **N3 — Job HANDLE accumulation over process lifetime.** Every `apply_rlimits` + `cmd.spawn()` creates a new Job Object whose HANDLE is intentionally never closed. For a long-running Tauri process that spawns thousands of agent children, this would eventually hit the per-process handle limit (default ~16M on modern Windows, far beyond practical agent throughput). Not a concern for C1 (child spawn count is bounded by agent concurrency). Flag for C4 review: consider a pool or reuse strategy if spawn rates increase.

- **N4 — Author identity mismatch.** The C1 commit author is `Saurias92 <gualtierimarco09@hotmail.com>`, while earlier commits on the `windows-port` branch (`H`, `A`, pre-existing) use `gualt <gualt@devboule.local>` and `cooperate <gualtiero.paride@gmail.com>`. This is consistent with a multi-machine setup (personal vs. work git config) and is not a blocker. Worth standardizing before merge to `main` if a single-author audit trail is desired.

- **N5 — C2 (Restricted Token) follow-up from this foundation is clearly possible.** The two-phase `apply_rlimits` → spawn → `attach_to_child` architecture is extensible: C2 will add a second kernel object (restricted token) created in the pre-spawn phase and assigned in the post-spawn phase, reusing the same `OpenProcess` pattern. The thread-local stash can be extended to hold a `(HANDLE, HANDLE)` tuple or a dedicated struct. No API redesign needed.

- **N6 — Two tests exist but limited coverage in CI.** `apply_rlimits_stashes_handle` (`windows.rs:120-131`) only asserts that `apply_rlimits` does not panic — the handle stash is opaque. `job_terminates_child_on_kill_on_close` (`windows.rs:133-147`) is `#[ignore]` and marked for manual Windows execution. This is consistent with the spec ("For v1: ship 1 test that exercises the happy path; the kill-on-close stress test is `#[ignore]`"). A future milestone should add a CI-safe integration test that spawns a short-lived child (`cmd /c exit 0`), attaches it to a job, and verifies the child exits cleanly — this would exercise the full create→stash→open→assign→child-exit path without the kill-on-close cross-process dependency.

## Verdict

✅ **PASS** — Implementation matches plan, compiles cleanly, no regressions, spec deviations are justified and correct.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "10 verification checks with file:line citations above; all pass. Spec deviations (AssignProcessToJobObject namespace + module path) verified correct against windows=0.58."
    }
  ],
  "changedFiles": [
    "src-tauri/src/backend/agentic_tools.rs",
    "src-tauri/src/backend/sandbox/mod.rs",
    "src-tauri/src/backend/sandbox/windows.rs"
  ],
  "testsAddedOrUpdated": [
    "src-tauri/src/backend/sandbox/windows.rs::apply_rlimits_stashes_handle (new)",
    "src-tauri/src/backend/sandbox/windows.rs::job_terminates_child_on_kill_on_close (new, #[ignore])"
  ],
  "commandsRun": [
    {
      "command": "git show 5510752 --stat",
      "result": "passed",
      "summary": "3 files, +178/-5, matches expected diff"
    },
    {
      "command": "git diff db0cb20 5510752 -- src-tauri/Cargo.toml",
      "result": "passed",
      "summary": "empty — Cargo.toml untouched"
    },
    {
      "command": "cargo check --tests --manifest-path src-tauri/Cargo.toml",
      "result": "passed",
      "summary": "0 errors, 168 pre-existing warnings only"
    },
    {
      "command": "grep TODO/unimplemented/todo in windows.rs",
      "result": "passed",
      "summary": "no implementation todos in the new file"
    }
  ],
  "validationOutput": [
    "is_enforced() returns false on Windows (mod.rs:216-222) — gated on C2+C3+C4 per FINAL plan line 42",
    "wrap() Windows arm calls crate::backend::sandbox::windows::wrap_policy (mod.rs:133-135)",
    "apply_rlimits Windows arm delegates to windows::apply_rlimits (mod.rs:200-203)",
    "windows.rs: 151 lines, 3 public fns, thread_local stash, intentional handle leak, no todos",
    "agentic_tools.rs:1066-1074: attach_to_child after spawn with graceful failure"
  ],
  "residualRisks": [
    "Thread-local stash is single-slot per thread — fragile if spawn sequence is ever reordered (N1)",
    "ProcessMemoryLimit semantics differ from Unix RLIMIT_AS (N2)",
    "Job HANDLE accumulation over long-lived process lifetime (N3)",
    "Kill-on-close test is #[ignore] and untested in CI (N6)"
  ],
  "noStagedFiles": true,
  "diffSummary": "C1 — Windows Job Object sandbox: creates kill-on-close + memory-limit job before cmd.spawn(), stashes HANDLE in thread-local, assigns child process post-spawn. is_enforced() stays false. 3 files, +178/-5.",
  "reviewFindings": [
    "no blockers"
  ],
  "manualNotes": "Author identity differs from earlier commits (Saurias92 vs gualt) — standardize git config before merge if single-author audit trail is desired. Two spec corrections by worker (AssignProcessToJobObject in JobObjects not Threading; crate::backend::sandbox::windows not super::windows) are verified correct."
}
```
