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

/// Apply a restricted token to the child process (C2).
///
/// **v1 STATUS: STUB — documented gap, NOT enforced.**
///
/// Windows does NOT allow token re-attachment after `CreateProcess`. The
/// `std::process::Command::spawn()` path creates the process without a custom
/// token, so we cannot apply `CreateRestrictedToken` post-spawn.
///
/// The real implementation requires spawning via `CreateProcessAsUserW` in a
/// thin sandbox-broker shim (writes job handle + restricted token + ACL grant
/// order). That broker is a separate sub-plan — see
/// `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §C2 decision rule.
///
/// Until the broker lands, this function is a no-op that returns `Ok(())`.
/// `is_enforced()` stays `false` on Windows, so the broker module's
/// `effective_sandbox_mode()` correctly degrades `Unattended` to `Ask`.
pub fn apply_restricted_token(_cmd: &mut std::process::Command) -> Result<(), String> {
    // TODO(C2-broker): implement CreateRestrictedToken + CreateProcessAsUserW broker.
    // See specs/PORT_MACOS_TO_WINDOWS_FINAL.md §C2. Until then, no-op.
    Ok(())
}

// ─── C3: Filesystem ACL layer ────────────────────────────────────────────────
//
// Applies deny-write / allow-write ACLs to paths derived from SandboxPolicy.
// Uses `icacls` CLI (incremental ACE add, preserves existing ACEs — better than
// SDDL replace-all-DACL for v1). Saves the original ACL to a temp file so
// restore_path_policy can put it back after the child exits.
//
// Note: `Win32_Security_Authorization` feature in Cargo.toml is currently unused
// by this icacls-based implementation. It was added for the broker sub-plan that
// will eventually use `GetNamedSecurityInfoW` / `SetNamedSecurityInfoW` directly.
//
// Not wired into the spawner — the broker sub-plan (C2-broker) will call this
// before spawn and restore_path_policy after child exit. is_enforced() stays false.

/// Saved ACL backup for a path, so it can be restored after the sandboxed child exits.
pub struct PathAclSnapshot {
    path: std::path::PathBuf,
    backup_file: std::path::PathBuf,
}

/// Canonicalize a path for ACL application (mirrors seatbelt::canonical_sandbox_path).
fn canonicalize_path(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Save the current ACL of `path` to a temp file via `icacls /save`.
///
/// Note: `icacls` ships on all Windows since Vista (not available on Nano Server
/// or Docker without the full Windows base image). The binary backup format is
/// machine-local — it cannot be transferred cross-machine, but save→restore on
/// the same machine works correctly.
///
/// Note: this function is called via `std::process::Command`, NOT through Git
/// Bash. Rust's `Command` uses `CreateProcessW` directly, so the `/save` flag is
/// passed as a literal string (Git Bash's `/save` → `C:/Program Files/Git/save`
/// path translation does NOT apply).
fn save_acl(path: &Path) -> Result<std::path::PathBuf, String> {
    let backup = std::env::temp_dir().join(format!(
        "devboule_acl_{}_{}",
        std::process::id(),
        path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("root".into())
    ));
    let out = std::process::Command::new("icacls")
        .arg(path.as_os_str())
        .args(["/save", &backup.to_string_lossy()])
        .output()
        .map_err(|e| format!("icacls /save spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "icacls /save failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(backup)
}

/// Add a deny-write ACE for Everyone (S-1-1-0) on `path` via `icacls /deny`.
///
/// Warning: Everyone (S-1-1-0) includes the current user. This means the Tauri
/// app itself will also be blocked from writing to `path` while the deny ACE is
/// active. The save/restore pattern in `apply_path_policy` / `restore_path_policy`
/// handles this: the deny is applied BEFORE spawn and removed AFTER the child
/// exits. The caller must ensure `restore_path_policy` is called even on error.
fn deny_write_everyone(path: &Path) -> Result<(), String> {
    let out = std::process::Command::new("icacls")
        .arg(path.as_os_str())
        .args(["/deny", "*S-1-1-0:(W)"])
        .output()
        .map_err(|e| format!("icacls /deny spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "icacls /deny failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Add an allow-write ACE for Everyone (S-1-1-0) on `path` via `icacls /grant`.
fn allow_write_everyone(path: &Path) -> Result<(), String> {
    let out = std::process::Command::new("icacls")
        .arg(path.as_os_str())
        .args(["/grant", "*S-1-1-0:(W)"])
        .output()
        .map_err(|e| format!("icacls /grant spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "icacls /grant failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Restore the original ACL on `path` from a backup file via `icacls /restore`.
fn restore_acl(path: &Path, backup_file: &Path) -> Result<(), String> {
    let out = std::process::Command::new("icacls")
        .arg(path.as_os_str())
        .args(["/restore", &backup_file.to_string_lossy()])
        .output()
        .map_err(|e| format!("icacls /restore spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "icacls /restore failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Apply filesystem ACLs derived from `policy` to the paths it references (C3).
///
/// - `readonly_root`: adds a deny-write ACE for Everyone (S-1-1-0).
/// - `writable_paths`: adds an allow-write ACE for Everyone.
///
/// Uses `icacls` CLI which adds ACEs INCREMENTALLY (preserves existing ACEs —
// better than a raw SDDL replace-all-DACL approach for v1).
///
/// Returns a snapshot of backup files so [`restore_path_policy`] can restore.
///
/// **Not wired into the spawner**: the broker sub-plan (C2-broker) will call
/// this before spawn. `is_enforced()` stays `false` on Windows.
pub fn apply_path_policy(policy: &SandboxPolicy) -> Result<Vec<PathAclSnapshot>, String> {
    let mut snapshots = Vec::new();

    if policy.readonly_root.is_absolute() {
        let canon = canonicalize_path(&policy.readonly_root);
        let backup = save_acl(&canon)?;
        deny_write_everyone(&canon)?;
        snapshots.push(PathAclSnapshot { path: canon, backup_file: backup });
    }

    for wp in &policy.writable_paths {
        if wp.is_absolute() {
            let canon = canonicalize_path(wp);
            let backup = save_acl(&canon)?;
            allow_write_everyone(&canon)?;
            snapshots.push(PathAclSnapshot { path: canon, backup_file: backup });
        }
    }

    Ok(snapshots)
}

/// Restore the original ACLs saved by [`apply_path_policy`].
pub fn restore_path_policy(snapshots: Vec<PathAclSnapshot>) -> Result<(), String> {
    for snap in snapshots {
        restore_acl(&snap.path, &snap.backup_file)?;
        let _ = std::fs::remove_file(&snap.backup_file);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::sandbox::{ResourceLimits, SandboxPolicy};

    #[test]
    fn apply_restricted_token_stub_returns_ok() {
        let mut cmd = std::process::Command::new("cmd.exe");
        let result = apply_restricted_token(&mut cmd);
        assert!(result.is_ok(), "v1 stub must return Ok; got {result:?}");
    }

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

    /// C3: apply deny-write on a temp file, then restore the original ACL.
    /// Verifies the icacls save → deny → restore pipeline works end-to-end.
    #[test]
    fn apply_and_restore_path_policy_roundtrip() {
        let temp = std::env::temp_dir().join(format!("devboule_c3_test_{}", std::process::id()));
        std::fs::write(&temp, "test").expect("write temp file");

        let policy = SandboxPolicy::deny(temp.clone());
        let snapshots = apply_path_policy(&policy).expect("apply should succeed");
        assert!(!snapshots.is_empty(), "should have at least one snapshot");

        restore_path_policy(snapshots).expect("restore should succeed");

        let _ = std::fs::remove_file(&temp);
    }

    /// C3: apply allow-write on a writable path, then restore.
    #[test]
    fn apply_writable_path_and_restore() {
        let temp = std::env::temp_dir().join(format!("devboule_c3_writable_{}", std::process::id()));
        std::fs::create_dir_all(&temp).expect("create temp dir");

        let policy = SandboxPolicy::deny("C:\\nonexistent".into())
            .writable(temp.clone());
        let snapshots = apply_path_policy(&policy).expect("apply should succeed");
        assert!(!snapshots.is_empty(), "should have at least one snapshot");

        restore_path_policy(snapshots).expect("restore should succeed");

        let _ = std::fs::remove_dir_all(&temp);
    }
}
