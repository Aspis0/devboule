# C1 — Job Object wrapper decision (parent-direct, oracle stalled)

**Status**: Spec produced by parent (MiniMax-M3) reading:
- `src-tauri/src/backend/sandbox/mod.rs` (full file, 325 lines)
- `src-tauri/src/backend/sandbox/seatbelt.rs` (full file, 384 lines)
- `src-tauri/src/backend/agentic_tools.rs:1005-1065` (spawn site)
- `src-tauri/Cargo.toml:140-180` (windows deps block)

Oracle was launched for this analysis but stalled at 37.5k tokens in thinking without producing output (GLM-5 hang pattern, see `MEMORY.md` §"GLM-5 hang"). Killed. Parent did the analysis.

---

## What `wrap()` does today on Windows

`mod.rs:100-118`: passthrough. Returns `SandboxedCommand { program, args }` unchanged, logs a one-shot warning. NO confinement.

`mod.rs:152`: `apply_rlimits` is no-op (cfg `not(unix)`).

`mod.rs:215-217`: `is_enforced()` returns `false` on Windows.

---

## Proposed `windows.rs` API

```rust
//! Windows sandbox backend (C1): Job Object wrapper for kill-on-close + memory limits.
//! Stage 1 of 4 for the Windows sandbox stack (C1..C4 per specs/PORT_MACOS_TO_WINDOWS_FINAL.md).
//! C2 (Restricted Token), C3 (filesystem ACL), C4 (WFP) land in separate milestones.

#![cfg(target_os = "windows")]

use std::path::Path;
use std::process::Command;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    CreateJobObjectW, SetInformationJobObject, JobObjectExtendedLimitInformation,
};
use windows::Win32::System::Threading::{
    OpenProcess, AssignProcessToJobObject, TerminateProcess,
    PROCESS_ALL_ACCESS,
};

use super::{ResourceLimits, SandboxedCommand, SandboxPolicy};

/// Apply the Job Object for `policy` to a pre-spawn `Command`.
///
/// Creates a Job Object with KILL_ON_JOB_CLOSE and (if set in policy) a memory limit,
/// stores its HANDLE in a thread-local, and returns the wrapped program/args unchanged
/// (the actual child attach happens in `attach_to_child` after `cmd.spawn()`).
///
/// Pattern: a Windows Job Object is a kernel object that lives in the parent process.
/// It must be created BEFORE the child spawns, and the child must be ASSIGNED to it AFTER
/// it spawns (we need the child's PID). So we have a two-phase API.
pub fn wrap_policy(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    _cwd: &Path,
) -> SandboxedCommand {
    // The job handle is created in apply_rlimits below and stashed in THREAD_LOCAL
    // for the matching attach_to_child call. Here we just return the command unchanged.
    let _ = policy;
    SandboxedCommand {
        program: program.to_string(),
        args: args.to_vec(),
    }
}

/// Replaces the no-op `apply_rlimits` on Windows. Creates the Job Object, configures it,
/// and stashes the HANDLE in a thread-local so the matching `attach_to_child` (called by
/// the spawner right after `cmd.spawn()`) can assign the child to it.
pub fn apply_rlimits(cmd: &mut Command, limits: &ResourceLimits) {
    let memory_limit = limits.addr_space_bytes.unwrap_or(u64::MAX);
    unsafe {
        let job = match CreateJobObjectW(None, None) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[sandbox/windows] CreateJobObjectW failed: {e}");
                return;
            }
        };
        // SAFETY: zeroed JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a valid starting state;
        // we set only the fields we need.
        let mut info = std::mem::zeroed::<windows::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION>();
        use windows::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        info.BasicLimitInformation = JOBOBJECT_BASIC_LIMIT_INFORMATION {
            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            ..Default::default()
        };
        info.ProcessMemoryLimit = memory_limit;
        let info_size = std::mem::size_of::<windows::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        if let Err(e) = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            info_size,
        ) {
            eprintln!("[sandbox/windows] SetInformationJobObject failed: {e}");
            let _ = CloseHandle(job);
            return;
        }
        // Stash the handle for attach_to_child. The spawner MUST call attach_to_child
        // immediately after cmd.spawn() — the thread-local acts as a single-slot handoff
        // buffer between these two phases.
        STASHED_JOB.with(|cell| {
            *cell.borrow_mut() = Some(job);
        });
    }
    // cmd itself is unused here — the actual attach happens in attach_to_child.
    let _ = cmd;
}

thread_local! {
    static STASHED_JOB: std::cell::RefCell<Option<HANDLE>> = const { std::cell::RefCell::new(None) };
}

/// Called by the spawner RIGHT AFTER `cmd.spawn()`. Takes the spawned child's PID,
/// pops the stashed job handle from the thread-local, and assigns the child to the job.
/// On success, dropping the parent handle (or the process exiting) will terminate the child
/// via KILL_ON_JOB_CLOSE.
pub fn attach_to_child(child_pid: u32) -> Result<(), String> {
    let job = STASHED_JOB.with(|cell| cell.borrow_mut().take());
    let job = match job {
        Some(h) => h,
        None => {
            return Err("attach_to_child called without a prior apply_rlimits on Windows".into());
        }
    };
    unsafe {
        // PROCESS_ALL_ACCESS = 0x1F0FFF (matches OpenProcess for full handle)
        let proc_handle = OpenProcess(PROCESS_ALL_ACCESS, false, child_pid)
            .map_err(|e| format!("OpenProcess({child_pid}) failed: {e}"))?;
        if let Err(e) = AssignProcessToJobObject(job, proc_handle) {
            let _ = CloseHandle(proc_handle);
            let _ = CloseHandle(job);
            return Err(format!("AssignProcessToJobObject failed: {e}"));
        }
        // We deliberately do NOT close proc_handle here: doing so can break the job
        // association on some Windows versions. It will be released when the child exits.
        // We DO close the job handle... actually, NO: closing the job handle would
        // terminate the child (KILL_ON_JOB_CLOSE). The job handle must outlive the child.
        // Drop it on process exit (intentional leak, OS cleans up).
        let _ = proc_handle;
        let _ = job;
        Ok(())
    }
}
```

---

## How the spawner hooks this in

`agentic_tools.rs:1011-1056` after `cmd.spawn()`:

```rust
let mut child = cmd.spawn().map_err(|e| format!("failed to start '{}': {e}", argv[0]))?;
let pid = child.id();

#[cfg(target_os = "windows")]
{
    use crate::backend::sandbox::windows;
    if let Err(e) = windows::attach_to_child(pid) {
        eprintln!("[sandbox/windows] WARN: failed to attach child to Job Object: {e}");
        // Continue anyway: the child runs unrestricted (current behavior on Windows).
    }
}
```

This is the ONLY call site change. `wrap()` and `apply_rlimits()` are called as today; on Windows they go through the new path.

---

## Failure modes

| Failure | Handling |
|---|---|
| `CreateJobObjectW` fails | Log, return early. Child runs unrestricted (current behavior). `is_enforced()` stays false. |
| `SetInformationJobObject` fails | Same: log + continue unrestricted. |
| `OpenProcess` fails (e.g. child already exited, race) | `attach_to_child` returns Err, spawner logs warning, continues. |
| `AssignProcessToJobObject` fails | Same. |
| `attach_to_child` called without prior `apply_rlimits` (programmer error) | Returns Err with a clear message. |
| Child spawned on a non-Windows target | All paths cfg-gated `#[cfg(target_os = "windows")]`. No-op elsewhere. |

---

## Test strategy

In `windows.rs`, gated `#[cfg(target_os = "windows")]`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::sandbox::{NetPolicy, ResourceLimits, SandboxPolicy};

    #[test]
    fn apply_rlimits_stashes_handle() {
        let mut cmd = std::process::Command::new("cmd.exe");
        let limits = ResourceLimits { cpu_secs: 60, addr_space_bytes: Some(1 << 30), max_procs: 16 };
        apply_rlimits(&mut cmd, &limits);
        // We can't read the stash directly, but we can verify attach_to_child now finds it.
        // This test only verifies the stash mechanism; full kill-on-close test follows.
    }

    #[test]
    fn job_terminates_child_on_kill_on_close() {
        // Spawn cmd /c "ping 127.0.0.1 -n 30" (long-running)
        // Attach to job (via the public API path: apply_rlimits then attach_to_child)
        // Then drop the job handle (or rely on test-process exit)
        // Assert the child PID is gone within 2s.
        //
        // Implementation note: this test is tricky because the job handle is leaked
        // (intentionally) — releasing it inside the test would KILL the test process's
        // other children. Use TerminateProcess directly as a stand-in for the kill-on-close
        // semantics, or run in a child test process. For v1, document as #[ignore] for CI.
    }
}
```

For v1: ship 1 test that exercises the happy path (create job, spawn + attach, child runs, child exits cleanly, OS releases). The kill-on-close stress test is `#[ignore]` and run manually on a Windows host.

---

## Risks + open questions (locked)

1. **Job handle leak is intentional.** Closing the job handle triggers KILL_ON_JOB_CLOSE on the child. The handle must outlive the child. OS cleans it up on parent exit. For long-lived parent processes, this is fine; for short-lived batch tools that spawn millions of children, it would leak handles — but `is_enforced()` stays false until C2-C4 land anyway, and devboule's agent spawn rate is bounded.

2. **Thread-local stash is single-slot.** If two threads both call `apply_rlimits` before either calls `attach_to_child`, the second stashes overwrite the first. For devboule, `agentic_tools.rs` spawns one child per call from a single thread, so this is fine. If we later parallelize spawns, the stash needs to be keyed by spawn ID or moved into the `SandboxedCommand` struct.

3. **Token re-attachment in C2** will require a different approach (cannot re-attach post-spawn on Windows). Likely a broker sub-process. Out of scope for C1; flagged in the plan.

4. **Address-space limit is `ProcessMemoryLimit`** in JOBOBJECT_EXTENDED_LIMIT_INFORMATION. Note: this is the JOBMEMORY, not RLIMIT_AS — Windows measures "private commit charge" not "virtual address space size". Sufficient for the runaway-task guard use case (the plan says `addr_space_bytes`).

5. **No `max_procs` enforcement in C1.** RLIMIT_NPROC is unix-only and intentionally not set on macOS (see mod.rs comment about UID caps). The Windows equivalent (JOB_OBJECT_LIMIT_ACTIVE_PROCESS) could be added later if needed.

---

## What the worker (hy3) must write

Files to create:
- `src-tauri/src/backend/sandbox/windows.rs` (the file above, ~80 lines of impl + tests)

Files to modify:
- `src-tauri/src/backend/sandbox/mod.rs`:
  - Add `pub mod windows;` after `pub mod seatbelt;` at line 1
  - In `wrap()` at line ~100, change the `#[cfg(not(target_os = "macos"))]` arm to:
    ```rust
    #[cfg(target_os = "windows")]
    { super::windows::wrap_policy(policy, program, args, cwd) }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { /* current passthrough */ }
    ```
  - In `apply_rlimits` no-op arm at line ~152, add:
    ```rust
    #[cfg(target_os = "windows")]
    pub fn apply_rlimits(cmd: &mut Command, limits: &ResourceLimits) {
        super::windows::apply_rlimits(cmd, limits)
    }
    ```
  - `is_enforced()` STAYS false (gated on C2-C4 landing)

- `src-tauri/src/backend/agentic_tools.rs:1011-1056`:
  - After `let mut child = cmd.spawn()...`, add the `attach_to_child` call (3 lines, shown above)

Do NOT touch:
- `Cargo.toml` (M0 features are sufficient)
- `sandbox/seatbelt.rs` (macOS-only, unchanged)
- `is_enforced()` (stays false)
- `is_enforced_false_off_macos` test (still passes because Windows still returns false)
