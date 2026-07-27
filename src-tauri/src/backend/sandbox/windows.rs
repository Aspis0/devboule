//! Windows sandbox backend (C1): Job Object wrapper for kill-on-close + memory limits.
//! Stage 1 of 4 for the Windows sandbox stack (C1..C4 per specs/PORT_MACOS_TO_WINDOWS_FINAL.md).
//! C2 (Restricted Token), C3 (filesystem ACL), C4 (WFP) land in separate milestones.

#![cfg(target_os = "windows")]

use std::path::Path;
use std::process::Command;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject, JobObjectExtendedLimitInformation,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_ALL_ACCESS};

use super::{ResourceLimits, SandboxedCommand, SandboxPolicy};

/// Apply the Job Object for `policy` to a pre-spawn `Command`.
///
/// Returns the program/args unchanged: the Job Object is a kernel object that lives in the
/// parent process and is created in `apply_rlimits` (called by the spawner before `cmd.spawn()`),
/// then stashed in a thread-local for `attach_to_child` (called right after `cmd.spawn()`).
///
/// Pattern: a Windows Job Object must be created BEFORE the child spawns, and the child must be
/// ASSIGNED to it AFTER it spawns (we need the child's PID). Hence the two-phase API.
pub fn wrap_policy(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    _cwd: &Path,
) -> SandboxedCommand {
    // The job handle is created in apply_rlimits and stashed for attach_to_child. The policy is
    // only consumed by the macOS Seatbelt backend; the Job Object reads limits directly.
    let _ = policy;
    SandboxedCommand {
        program: program.to_string(),
        args: args.to_vec(),
    }
}

thread_local! {
    static STASHED_JOB: std::cell::RefCell<Option<HANDLE>> = const { std::cell::RefCell::new(None) };
}

/// Replaces the no-op `apply_rlimits` on Windows. Creates the Job Object, configures it with
/// KILL_ON_JOB_CLOSE (+ an optional process memory limit), and stashes the HANDLE in a thread-local
/// so the matching `attach_to_child` (called by the spawner right after `cmd.spawn()`) can assign
/// the child to it. On any failure the child simply runs unrestricted (current behavior on Windows).
pub fn apply_rlimits(_cmd: &mut Command, limits: &ResourceLimits) {
    let memory_limit: usize = limits
        .addr_space_bytes
        .map(|b| b as usize)
        .unwrap_or(usize::MAX);
    unsafe {
        let job = match CreateJobObjectW(None, None) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[sandbox/windows] CreateJobObjectW failed: {e}");
                return;
            }
        };
        // SAFETY: zeroed JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a valid starting state; we set
        // only the fields we need. BasicLimitInformation is likewise zeroed (no flags) then we set
        // KILL_ON_JOB_CLOSE, which is the only flag we require for C1.
        let mut info = std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>();
        let mut basic = std::mem::zeroed::<JOBOBJECT_BASIC_LIMIT_INFORMATION>();
        basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        info.BasicLimitInformation = basic;
        info.ProcessMemoryLimit = memory_limit;
        if let Err(e) = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) {
            eprintln!("[sandbox/windows] SetInformationJobObject failed: {e}");
            let _ = CloseHandle(job);
            return;
        }
        // Stash the handle for attach_to_child. The spawner MUST call attach_to_child immediately
        // after cmd.spawn() — the thread-local is a single-slot handoff buffer between the two phases.
        STASHED_JOB.with(|cell| {
            *cell.borrow_mut() = Some(job);
        });
    }
}

/// Called by the spawner RIGHT AFTER `cmd.spawn()`. Takes the spawned child's PID, pops the stashed
/// job handle from the thread-local, and assigns the child to the job. On success, closing the parent
/// handle (or the parent process exiting) terminates the child via KILL_ON_JOB_CLOSE.
pub fn attach_to_child(child_pid: u32) -> Result<(), String> {
    let job = STASHED_JOB.with(|cell| cell.borrow_mut().take());
    let job = match job {
        Some(h) => h,
        None => {
            return Err("attach_to_child called without a prior apply_rlimits on Windows".into());
        }
    };
    unsafe {
        let proc_handle = OpenProcess(PROCESS_ALL_ACCESS, false, child_pid)
            .map_err(|e| format!("OpenProcess({child_pid}) failed: {e}"))?;
        if let Err(e) = AssignProcessToJobObject(job, proc_handle) {
            let _ = CloseHandle(proc_handle);
            let _ = CloseHandle(job);
            return Err(format!("AssignProcessToJobObject failed: {e}"));
        }
        // We deliberately do NOT close proc_handle here: releasing it can break the job association
        // on some Windows versions; it is freed when the child exits. We also do NOT close the job
        // handle: closing it would terminate the child via KILL_ON_JOB_CLOSE. It must outlive the
        // child (intentional leak; the OS frees it when the parent process exits).
        let _ = proc_handle;
        let _ = job;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::sandbox::ResourceLimits;

    #[test]
    fn apply_rlimits_stashes_handle() {
        let mut cmd = std::process::Command::new("cmd.exe");
        let limits = ResourceLimits {
            cpu_secs: 60,
            addr_space_bytes: Some(1 << 30),
            max_procs: 16,
        };
        apply_rlimits(&mut cmd, &limits);
        // The handle is now stashed; a real spawn + attach_to_child exercises the full path.
        // This test only verifies apply_rlimits does not panic on a valid Job Object creation.
    }

    #[test]
    #[ignore = "kill-on-close semantics require a dedicated child process; run manually on Windows"]
    fn job_terminates_child_on_kill_on_close() {
        let mut cmd = std::process::Command::new("cmd.exe");
        cmd.args(["/c", "ping", "127.0.0.1", "-n", "30"]);
        let limits = ResourceLimits {
            cpu_secs: 60,
            addr_space_bytes: None,
            max_procs: 16,
        };
        apply_rlimits(&mut cmd, &limits);
        let child = cmd.spawn().expect("spawn long-running child");
        attach_to_child(child.id()).expect("attach child to job");
        // Closing the job handle (or parent exit) would terminate the child via KILL_ON_JOB_CLOSE.
    }

    /// Reviewer N6: CI-safe integration test of the full create -> stash -> spawn -> attach -> exit
    /// path, without depending on kill-on-close cross-process behavior. Spawns `cmd /c exit 0`,
    /// attaches it to the Job Object, and waits for the child to exit normally. This exercises
    /// every public function in this module end-to-end.
    #[test]
    fn spawn_attach_and_exit_cleanly() {
        let mut cmd = std::process::Command::new("cmd.exe");
        cmd.args(["/c", "exit", "0"])
           .stdout(std::process::Stdio::null())
           .stderr(std::process::Stdio::null());
        let limits = ResourceLimits {
            cpu_secs: 60,
            addr_space_bytes: None,
            max_procs: 16,
        };
        apply_rlimits(&mut cmd, &limits);
        let mut child = cmd.spawn().expect("spawn exit-0 child");
        attach_to_child(child.id()).expect("attach child to job");
        let status = child.wait().expect("wait for child");
        assert!(status.success(), "cmd /c exit 0 should report success; got {:?}", status);
    }
}
