//! Windows sandbox backend (C1): Job Object wrapper for kill-on-close + memory limits.
//! Stage 1 of 4 for the Windows sandbox stack (C1..C4 per specs/PORT_MACOS_TO_WINDOWS_FINAL.md).
//! C2 (Restricted Token), C3 (filesystem ACL), C4 (WFP) land in separate milestones.

#![cfg(target_os = "windows")]

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, WIN32_ERROR};
use windows::Win32::Security::{
    AllocateAndInitializeSid, CreateRestrictedToken, FreeSid, GetSecurityDescriptorLength,
    OBJECT_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SID_IDENTIFIER_AUTHORITY,
    SID_AND_ATTRIBUTES, TOKEN_ACCESS_MASK, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY,
    CREATE_RESTRICTED_TOKEN_FLAGS, DACL_SECURITY_INFORMATION, SECURITY_NT_AUTHORITY,
};
use windows::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, ACCESS_MODE,
    EXPLICIT_ACCESS_W, DENY_ACCESS, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT,
    TRUSTEE_FORM, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_TYPE, TRUSTEE_W,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_LIMIT_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_LIMIT_PROCESS_TIME,
};
const SECURITY_RESTRICTED_CODE_RID: u32 = 12;
use windows::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_ALL_ACCESS};
use windows::Win32::Security::SetFileSecurityW;
use windows::Win32::Storage::FileSystem::{
    FILE_DELETE_CHILD, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};

use super::{ResourceLimits, SandboxPolicy, SandboxedCommand};

/// Apply the Job Object for `policy` to a pre-spawn `Command`.
///
/// Returns the program/args unchanged: the Job Object is a kernel object that lives in the
/// parent process and is created in `apply_rlimits` (called by the spawner before `cmd.spawn()`),
/// then stashed in a thread-local for `attach_to_child` (called right after `cmd.spawn()`).
///
/// Pattern: a Windows Job Object must be created BEFORE the child spawns, and the child must be
/// ASSIGNED to it AFTER it spawns (we need the child's PID). Hence the two-phase API.
/// M-9: superseded by the broker's create_job_object. Kept for mod.rs public API.
#[allow(dead_code)]
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
/// M-9: superseded by the broker's create_job_object. Kept for the mod.rs public API
/// but NOT called by the production run_windows path. Do NOT extend this.
#[allow(dead_code)]
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
        // H-2 fix: enable PROCESS_MEMORY only when a finite limit is set.
        if limits.addr_space_bytes.is_some() {
            basic.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        }
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
/// M-9: superseded by the broker's CREATE_SUSPENDED + AssignProcessToJobObject.
#[allow(dead_code)]
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
#[derive(Clone)]
pub struct PathAclSnapshot {
    path: std::path::PathBuf,
    backup_file: std::path::PathBuf,
}

/// Canonicalize a path for ACL application (mirrors seatbelt::canonical_sandbox_path).
fn canonicalize_path(path: &Path) -> Result<std::path::PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("sandbox path must be absolute: {}", path.display()));
    }
    std::fs::canonicalize(path).map_err(|e| {
        format!(
            "sandbox path must exist and resolve before ACL application ({}): {e}",
            path.display()
        )
    })
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
    // C-2 fix: use a unique counter to prevent backup file collision when the same
    // path appears as both readonly_root and a writable_path.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let backup = std::env::temp_dir().join(format!(
        "devboule_acl_{}_{}_{}",
        std::process::id(),
        id,
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or("root".into())
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
        // H-7 fix: (OI)(CI) for inheritance to new files/subdirs, /T for existing ones.
        // C3 fix: use (WD,AD,WA,WEA,DC) instead of (W) — FILE_GENERIC_WRITE includes
        // SYNCHRONIZE, so icacls locks ITSELF out of the directory right after applying
        // the deny ACE (it can no longer enumerate path\* with /T). Explicit write
        // rights + DELETE_CHILD keep the directory enumerable while still denying all
        // writes and child deletion (verified on Win11, non-elevated shell).
        .args(["/deny", "*S-1-1-0:(OI)(CI)(WD,AD,WA,WEA,DC)", "/T"])
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
        // H-7 fix: (OI)(CI) for inheritance, /T for existing files/subdirs.
        .args(["/grant", "*S-1-1-0:(OI)(CI)(W)", "/T"])
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
    let mut processed: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();

    // Every mutation must be rolled back if a later path fails. Returning an
    // error without restoring earlier ACLs would leave the host modified while
    // the child was never spawned.
    let rollback = |snapshots: &mut Vec<PathAclSnapshot>| {
        let pending = std::mem::take(snapshots);
        let _ = restore_path_policy(pending);
    };

    // C-2 fix: deduplicate paths to prevent deny+allow collision on the same path.
    // If readonly_root also appears in writable_paths, only the deny-write is applied.
    if policy.readonly_root.is_absolute() {
        let canon = canonicalize_path(&policy.readonly_root)?;
        if processed.insert(canon.clone()) {
            let backup = match save_acl(&canon) {
                Ok(backup) => backup,
                Err(e) => {
                    rollback(&mut snapshots);
                    return Err(e);
                }
            };
            if let Err(e) = deny_write_everyone(&canon) {
                let _ = restore_acl(&canon, &backup);
                let _ = std::fs::remove_file(&backup);
                rollback(&mut snapshots);
                return Err(e);
            }
            snapshots.push(PathAclSnapshot {
                path: canon,
                backup_file: backup,
            });
        }
    }

    for wp in &policy.writable_paths {
        if wp.is_absolute() {
            let canon = canonicalize_path(wp)?;
            if processed.insert(canon.clone()) {
                let backup = match save_acl(&canon) {
                    Ok(backup) => backup,
                    Err(e) => {
                        rollback(&mut snapshots);
                        return Err(e);
                    }
                };
                if let Err(e) = allow_write_everyone(&canon) {
                    let _ = restore_acl(&canon, &backup);
                    let _ = std::fs::remove_file(&backup);
                    rollback(&mut snapshots);
                    return Err(e);
                }
                snapshots.push(PathAclSnapshot {
                    path: canon,
                    backup_file: backup,
                });
            }
        }
    }

    Ok(snapshots)
}

/// Restore the original ACLs saved by [`apply_path_policy`].
pub fn restore_path_policy(snapshots: Vec<PathAclSnapshot>) -> Result<(), String> {
    let (_, error) = restore_path_policy_with_remaining(snapshots);
    error.map_or(Ok(()), Err)
}

/// Restore ACLs while retaining snapshots whose restore failed. This lets the
/// owning child retry from `Drop` without retrying already-restored paths whose
/// backup files have been deleted.
fn restore_path_policy_with_remaining(
    snapshots: Vec<PathAclSnapshot>,
) -> (Vec<PathAclSnapshot>, Option<String>) {
    let mut remaining = Vec::new();
    let mut errors = Vec::new();
    for snap in snapshots {
        if let Err(e) = restore_acl(&snap.path, &snap.backup_file) {
            errors.push(format!("{}: {e}", snap.path.display()));
            remaining.push(snap);
        } else {
            let _ = std::fs::remove_file(&snap.backup_file);
        }
    }
    let error = if errors.is_empty() {
        None
    } else {
        Some(format!(
            "restore failed for {} paths: {}",
            errors.len(),
            errors.join("; ")
        ))
    };
    (remaining, error)
}

// ─── C3-restricted: Filesystem ACL layer using WinRestrictedCodeSid (S-1-5-12) ───────
//
// Replaces the Everyone-based deny/grant ACLs with a restricted-SID ACL mode.
// The child process runs with a restricted token containing WinRestrictedCodeSid,
// so the second access check evaluates this SID instead of the user's groups.
// We grant this SID read+execute on explicit read roots and modify on writable paths,
// with inheritance. Unspecified paths become inaccessible because the restricted SID
// has no grant on them.
//
// This uses SetNamedSecurityInfoW directly (not icacls) for precise control and
// to avoid the Everyone deny/grant side effects on the host process.

/// Saved security descriptor for a path, so it can be restored after the sandboxed child exits.
#[derive(Clone)]
pub struct PathSecuritySnapshot {
    path: std::path::PathBuf,
    sd_backup: Vec<u8>, // raw security descriptor bytes
}

/// Owns a temporary well-known SID until the ACL transaction completes.
/// Keeping this RAII-owned prevents leaks on every early-return path.
struct RestrictedSidGuard {
    sid: Option<PSID>,
}

impl RestrictedSidGuard {
    fn new(sid: PSID) -> Self {
        Self { sid: Some(sid) }
    }

    fn sid(&self) -> PSID {
        self.sid.expect("restricted SID guard is armed")
    }

    fn disarm(mut self) {
        self.sid.take();
    }
}

impl Drop for RestrictedSidGuard {
    fn drop(&mut self) {
        if let Some(sid) = self.sid.take() {
            unsafe {
                let _ = FreeSid(sid);
            }
        }
    }
}

/// Create the well-known WinRestrictedCodeSid (S-1-5-12).
/// The returned PSID is owned by the caller and must remain alive while it is used.
fn create_restricted_code_sid() -> Result<PSID, String> {
    let mut restricted_sid: PSID = PSID::default();
    let mut sid_authority = SID_IDENTIFIER_AUTHORITY {
        Value: [0, 0, 0, 0, 0, 5], // SECURITY_NT_AUTHORITY
    };
    unsafe {
        AllocateAndInitializeSid(
            &mut sid_authority,
            1,
            SECURITY_RESTRICTED_CODE_RID, // 12
            0, 0, 0, 0, 0, 0, 0,
            &mut restricted_sid,
        )
        .map_err(|e| format!("AllocateAndInitializeSid(WinRestrictedCodeSid) failed: {e}"))?;
    }
    Ok(restricted_sid)
}

/// Save the current security descriptor (DACL) of `path` to a byte vector.
fn save_security_descriptor(path: &Path) -> Result<Vec<u8>, String> {
    let mut sd_ptr = PSECURITY_DESCRIPTOR::default();
    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let result = GetNamedSecurityInfoW(
            windows::core::PCWSTR(path_wide.as_ptr()),
            SE_FILE_OBJECT,
            OBJECT_SECURITY_INFORMATION(DACL_SECURITY_INFORMATION.0),
            None,
            None,
            None,
            None,
            &mut sd_ptr,
        );
        if result != WIN32_ERROR(0) {
            return Err(format!("GetNamedSecurityInfoW failed: {result:?}"));
        }
        // Copy the security descriptor to our own buffer.
        let sd_len = GetSecurityDescriptorLength(sd_ptr) as usize;
        let mut sd_backup = vec![0u8; sd_len];
        std::ptr::copy_nonoverlapping(sd_ptr.0 as *const u8, sd_backup.as_mut_ptr(), sd_len);
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(sd_ptr.0));
        Ok(sd_backup)
    }
}

/// Restore a security descriptor to `path` from a byte vector.
fn restore_security_descriptor(path: &Path, sd_backup: &[u8]) -> Result<(), String> {
    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        // `sd_backup` is the self-relative descriptor returned by
        // GetNamedSecurityInfoW, not an ACL. Restore it with SetFileSecurityW;
        // passing the descriptor as a PACL would corrupt the target DACL.
        let result = SetFileSecurityW(
            windows::core::PCWSTR(path_wide.as_ptr()),
            OBJECT_SECURITY_INFORMATION(DACL_SECURITY_INFORMATION.0),
            PSECURITY_DESCRIPTOR(sd_backup.as_ptr() as *mut _),
        );
        if !result.as_bool() {
            return Err(format!("SetFileSecurityW restore failed: {:?}", windows::Win32::Foundation::GetLastError()));
        }
    }
    Ok(())
}

/// Build and apply a DACL granting the restricted SID specific access rights on `path`.
/// `access_mask`: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE for read roots,
/// FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE for writable paths.
/// `is_directory`: if true, apply inheritance (OI)(CI) for new files/subdirs.
fn apply_restricted_sid_acl(
    path: &Path,
    restricted_sid: PSID,
    access_mode: ACCESS_MODE,
    access_mask: u32,
    is_directory: bool,
) -> Result<(), String> {
    // Preserve the existing DACL and append only the restricted-SID grant.
    // Replacing the DACL would unnecessarily remove the host's explicit and
    // inherited entries while the child is running.
    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut old_acl: *mut windows::Win32::Security::ACL = std::ptr::null_mut();
    let mut old_sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        let result = GetNamedSecurityInfoW(
            windows::core::PCWSTR(path_wide.as_ptr()),
            SE_FILE_OBJECT,
            OBJECT_SECURITY_INFORMATION(DACL_SECURITY_INFORMATION.0),
            None,
            None,
            Some(&mut old_acl),
            None,
            &mut old_sd,
        );
        if result != WIN32_ERROR(0) {
            return Err(format!("GetNamedSecurityInfoW ACL failed: {result:?}"));
        }
    }

    // Build EXPLICIT_ACCESS for the restricted SID.
    let mut ea = EXPLICIT_ACCESS_W::default();
    ea.grfAccessPermissions = access_mask;
    ea.grfAccessMode = access_mode;
    ea.grfInheritance = if is_directory {
        windows::Win32::Security::SUB_CONTAINERS_AND_OBJECTS_INHERIT
    } else {
        windows::Win32::Security::NO_INHERITANCE
    };
    ea.Trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_FORM(TRUSTEE_IS_SID.0),
        TrusteeType: TRUSTEE_TYPE(TRUSTEE_IS_WELL_KNOWN_GROUP.0),
        ptstrName: windows::core::PWSTR(restricted_sid.0 as *mut u16),
    };

    // Create the new ACL from the explicit access entry and the original DACL.
    let mut new_acl: *mut windows::Win32::Security::ACL = std::ptr::null_mut();
    let result = unsafe {
        SetEntriesInAclW(
            Some(std::slice::from_ref(&ea)),
            (!old_acl.is_null()).then_some(old_acl as *const windows::Win32::Security::ACL),
            &mut new_acl,
        )
    };
    unsafe {
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(old_sd.0));
    }
    if result != WIN32_ERROR(0) {
        return Err(format!("SetEntriesInAclW failed: {result:?}"));
    }

    // Apply the new DACL to the path.
    let result = unsafe {
        SetNamedSecurityInfoW(
            windows::core::PCWSTR(path_wide.as_ptr()),
            SE_FILE_OBJECT,
            OBJECT_SECURITY_INFORMATION(DACL_SECURITY_INFORMATION.0),
            None,
            None,
            Some(new_acl as *const windows::Win32::Security::ACL),
            None,
        )
    };
    unsafe {
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(new_acl as *mut _));
    }
    if result != WIN32_ERROR(0) {
        return Err(format!(
            "SetNamedSecurityInfoW apply failed for {}: {result:?}",
            path.display()
        ));
    }
    Ok(())
}

/// Apply the restricted-SID ACL policy for the broker spawn.
///
/// Grants the restricted SID:
///   - Read+Execute on read roots (readonly_root, cwd, executable parent dir)
///   - Read+Write+Execute (Modify) on writable paths
///
/// Handles overlap: writable wins (grants more access).
/// Deduplicates canonical paths.
/// Rollback on failures.
/// Returns snapshots for restoration.
fn apply_restricted_sid_policy(
    policy: &SandboxPolicy,
    cwd: &Path,
    program: &str,
) -> Result<Vec<PathSecuritySnapshot>, String> {
    let mut snapshots = Vec::new();
    let mut processed: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();

    // Create the restricted SID once and keep it alive through the full ACL transaction.
    let restricted_sid = RestrictedSidGuard::new(create_restricted_code_sid()?);

    // Collect read roots: readonly_root, cwd, and the executable parent.
    // Canonicalization failures roll back every ACL already applied in this
    // transaction; they never return through `?` with host ACLs left changed.
    let mut read_roots = Vec::new();
    let mut collect_read_root = |candidate: &Path| -> Result<(), String> {
        let canonical = canonicalize_path(candidate)?;
        if !read_roots.contains(&canonical) {
            read_roots.push(canonical);
        }
        Ok(())
    };
    if policy.readonly_root.is_absolute() {
        if let Err(e) = collect_read_root(&policy.readonly_root) {
            let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
            return Err(e);
        }
    }
    if cwd.is_absolute() {
        if let Err(e) = collect_read_root(cwd) {
            let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
            return Err(e);
        }
    }
    if let Some(parent) = Path::new(program).parent() {
        if parent.is_absolute() {
            if let Err(e) = collect_read_root(parent) {
                let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                return Err(e);
            }
        }
    }
    // Normal Windows executables load system DLLs even when the executable
    // itself lives in a project/temp directory. Grant the restricted SID
    // read+execute on the system roots required for process startup; this is
    // not a user-home grant and remains read-only.
    if let Ok(system_root) = std::env::var("SystemRoot") {
        let system_root = PathBuf::from(system_root);
        for system_path in [system_root.clone(), system_root.join("System32")] {
            if system_path.is_dir() {
                if let Err(e) = collect_read_root(&system_path) {
                    let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                    return Err(e);
                }
            }
        }
    }

    // Apply read+execute on read roots.
    for root in read_roots {
        if processed.insert(root.clone()) {
            let sd_backup = match save_security_descriptor(&root) {
                Ok(sd_backup) => sd_backup,
                Err(e) => {
                    let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                    return Err(e);
                }
            };
            if let Err(e) = apply_restricted_sid_acl(&root, restricted_sid.sid(), GRANT_ACCESS, FILE_GENERIC_READ.0 | FILE_GENERIC_EXECUTE.0, root.is_dir()) {
                let _ = restore_security_descriptor(&root, &sd_backup);
                let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                return Err(e);
            }
            snapshots.push(PathSecuritySnapshot { path: root, sd_backup });
        }
    }

    // Apply read+write+execute (modify) on writable paths.
    // Writable wins over read-only if paths overlap.
    for wp in &policy.writable_paths {
        if wp.is_absolute() {
            let canon = match canonicalize_path(wp) {
                Ok(canon) => canon,
                Err(e) => {
                    let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                    return Err(e);
                }
            };
            // If already processed as read root, we need to upgrade the ACL.
            // We restore the original first, then apply the more permissive ACL.
            let is_upgrade = processed.contains(&canon);
            if is_upgrade {
                // Find and remove the old snapshot so we can re-apply.
                if let Some(idx) = snapshots.iter().position(|s| s.path == canon) {
                    let old_snap = snapshots.swap_remove(idx);
                    if let Err(e) = restore_security_descriptor(&old_snap.path, &old_snap.sd_backup) {
                        snapshots.push(old_snap);
                        let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                        return Err(e);
                    }
                }
                processed.remove(&canon);
            }
            if processed.insert(canon.clone()) {
                let sd_backup = match save_security_descriptor(&canon) {
                    Ok(sd_backup) => sd_backup,
                    Err(e) => {
                        let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                        return Err(e);
                    }
                };
                let access = FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0 | FILE_GENERIC_EXECUTE.0;
                if let Err(e) = apply_restricted_sid_acl(&canon, restricted_sid.sid(), GRANT_ACCESS, access, canon.is_dir()) {
                    let _ = restore_security_descriptor(&canon, &sd_backup);
                    let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                    return Err(e);
                }
                snapshots.push(PathSecuritySnapshot { path: canon, sd_backup });
            }
        }
    }

    // Protect existing repository control directories even when their parent
    // is writable. Windows ACLs have no pathname-regex deny, so apply a
    // restricted-SID deny ACE to the existing `.git`/`.devboule` trees.
    let mut protected = std::collections::HashSet::new();
    for wp in &policy.writable_paths {
        if !wp.is_absolute() {
            continue;
        }
        let writable_root = match canonicalize_path(wp) {
            Ok(path) => path,
            Err(e) => {
                let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                return Err(e);
            }
        };
        for name in [".git", ".devboule"] {
            let candidate = writable_root.join(name);
            if !candidate.exists() {
                continue;
            }
            let candidate = match canonicalize_path(&candidate) {
                Ok(path) => path,
                Err(e) => {
                    let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                    return Err(e);
                }
            };
            let mut stack = vec![candidate];
            while let Some(path) = stack.pop() {
                if !protected.insert(path.clone()) {
                    continue;
                }
                let is_directory = match std::fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata.file_type().is_dir(),
                    Err(e) => {
                        let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                        return Err(format!("inspect protected path {}: {e}", path.display()));
                    }
                };
                let sd_backup = match save_security_descriptor(&path) {
                    Ok(sd_backup) => sd_backup,
                    Err(e) => {
                        let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                        return Err(e);
                    }
                };
                let deny_mask = FILE_GENERIC_WRITE.0 | FILE_DELETE_CHILD.0 | 0x0001_0000;
                if let Err(e) = apply_restricted_sid_acl(
                    &path,
                    restricted_sid.sid(),
                    DENY_ACCESS,
                    deny_mask,
                    is_directory,
                ) {
                    let _ = restore_security_descriptor(&path, &sd_backup);
                    let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                    return Err(e);
                }
                snapshots.push(PathSecuritySnapshot { path: path.clone(), sd_backup });
                if is_directory {
                    let entries = match std::fs::read_dir(&path) {
                        Ok(entries) => entries,
                        Err(e) => {
                            let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                            return Err(format!("enumerate protected path {}: {e}", path.display()));
                        }
                    };
                    for entry in entries {
                        let entry = match entry {
                            Ok(entry) => entry,
                            Err(e) => {
                                let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                                return Err(format!("enumerate protected path {}: {e}", path.display()));
                            }
                        };
                        if let Ok(metadata) = std::fs::symlink_metadata(entry.path()) {
                            if !metadata.file_type().is_symlink() {
                                stack.push(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }

    // Success: release the SID after all ACLs have been set.
    restricted_sid.disarm();
    Ok(snapshots)
}

/// Restore security descriptors while retaining entries that failed. The owner
/// can retry the returned snapshots during Drop without losing rollback state.
fn restore_restricted_sid_policy_with_remaining(
    snapshots: Vec<PathSecuritySnapshot>,
) -> (Vec<PathSecuritySnapshot>, Option<String>) {
    let mut remaining = Vec::new();
    let mut errors = Vec::new();
    for snap in snapshots {
        if let Err(e) = restore_security_descriptor(&snap.path, &snap.sd_backup) {
            errors.push(format!("{}: {e}", snap.path.display()));
            remaining.push(snap);
        }
    }
    let error = if errors.is_empty() {
        None
    } else {
        Some(format!("restore failed for {} paths: {}", errors.len(), errors.join("; ")))
    };
    (remaining, error)
}

/// Restore security descriptors saved by `apply_restricted_sid_policy`.
fn restore_restricted_sid_policy(snapshots: Vec<PathSecuritySnapshot>) -> Result<(), String> {
    let (_, error) = restore_restricted_sid_policy_with_remaining(snapshots);
    error.map_or(Ok(()), Err)
}


// ─── C4: Network egress layer ──────────────────────────────────────────────────
//
// Blocks outbound network for the child's program path via `netsh advfirewall`.
// Per-application (not per-process), but real enforcement. Rule is added before
// spawn and removed after child exit. For v1: NetPolicy::None only. Loopback and
// Enabled are deferred (WFP filter complexity is out of v1 scope).

/// Saved firewall rule name for restore after child exit.
#[derive(Clone, Default)]
pub struct NetPolicySnapshot {
    rule_name: String,
}

/// Path to this process's firewall-rule journal (one rule name per line).
/// Used by [`cleanup_orphaned_firewall_rules`] to recover from crashes.
fn firewall_journal_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "devboule_firewall_journal_{}.txt",
        std::process::id()
    ))
}

/// Append a rule name to this process's journal.
fn firewall_journal_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

fn journal_add(rule: &str) {
    use std::io::Write;
    let _guard = firewall_journal_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(firewall_journal_path())
    {
        let _ = writeln!(f, "{rule}");
    }
}

/// Remove a rule name from this process's journal (rewrite without it).
fn journal_remove(rule: &str) {
    let _guard = firewall_journal_lock().lock().unwrap_or_else(|e| e.into_inner());
    let path = firewall_journal_path();
    if let Ok(lines) = std::fs::read_to_string(&path) {
        let kept: Vec<&str> = lines.lines().filter(|l| *l != rule).collect();
        if kept.is_empty() {
            let _ = std::fs::remove_file(&path);
        } else {
            let _ = std::fs::write(&path, kept.join("\n") + "\n");
        }
    }
}

/// M-10: remove firewall rules left behind by crashed devboule instances.
/// Scan temp dir for `devboule_firewall_journal_*.txt`, and for each whose
/// owning PID is no longer alive, delete the listed rules and the journal.
/// Safe to call at app startup.
pub fn cleanup_orphaned_firewall_rules() {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let tmp = std::env::temp_dir();
    let Ok(entries) = std::fs::read_dir(&tmp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(pid_str) = name
            .strip_prefix("devboule_firewall_journal_")
            .and_then(|s| s.strip_suffix(".txt"))
        else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        // Skip journals of still-running processes.
        // MEDIUM fix: close the OpenProcess handle — previously .is_ok() leaked it.
        let alive = unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => {
                    let _ = CloseHandle(h);
                    true
                }
                Err(_) => false,
            }
        };
        if alive {
            continue;
        }
        // Orphan: serialize journal read/delete against add/remove in this process.
        let _guard = firewall_journal_lock().lock().unwrap_or_else(|e| e.into_inner());
        let journal = entry.path();
        if let Ok(lines) = std::fs::read_to_string(&journal) {
            for rule in lines.lines() {
                let _ = std::process::Command::new("netsh")
                    .args([
                        "advfirewall",
                        "firewall",
                        "delete",
                        "rule",
                        &format!("name={rule}"),
                    ])
                    .output();
            }
        }
        let _ = std::fs::remove_file(&journal);
    }
}

/// Add a firewall rule blocking outbound network for `program` (C4).
/// Only applies when `policy.net == NetPolicy::None`.
///
/// H-1: `netsh advfirewall` requires administrator privileges. If the calling
/// process is not elevated, this returns an error directing the user to run as
/// admin. A v2 alternative (WFP filter via a broker service) would remove the
/// elevation requirement but is out of scope for v1.
pub fn apply_net_policy(
    policy: &SandboxPolicy,
    program: &str,
) -> Result<NetPolicySnapshot, String> {
    use super::NetPolicy;
    match policy.net {
        NetPolicy::None => {
            use std::sync::atomic::{AtomicU64, Ordering};
            static RULE_SEQ: AtomicU64 = AtomicU64::new(1);
            let rule_name = format!(
                "devboule_sandbox_block_{}_{}",
                std::process::id(),
                RULE_SEQ.fetch_add(1, Ordering::Relaxed)
            );
            let out = std::process::Command::new("netsh")
                .args([
                    "advfirewall",
                    "firewall",
                    "add",
                    "rule",
                    &format!("name={rule_name}"),
                    "dir=out",
                    "action=block",
                    &format!("program=\"{program}\""),
                ])
                .output()
                .map_err(|e| format!("netsh add rule spawn failed: {e}"))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let stdout = String::from_utf8_lossy(&out.stdout);
                let combined = format!("{stderr} {stdout}");
                // H-1: detect access-denied (admin required) and give a clear message.
                if combined.to_ascii_lowercase().contains("administrator")
                    || combined.to_ascii_lowercase().contains("access is denied")
                    || combined.to_ascii_lowercase().contains("elevation")
                {
                    return Err("network blocking requires administrator privileges \
                         (netsh advfirewall). Run devboule as administrator. \
                         See H-1 in specs/PORT_MACOS_TO_WINDOWS_FINAL.md."
                        .into());
                }
                return Err(format!("netsh add rule failed: {combined}"));
            }
            // M-10: record the rule so a crash can be recovered.
            journal_add(&rule_name);
            Ok(NetPolicySnapshot { rule_name })
        }
        NetPolicy::Loopback => Err(
            "Windows loopback-only network policy is not implemented; refusing an unrestricted spawn"
                .into(),
        ),
        NetPolicy::Enabled => Ok(NetPolicySnapshot {
            rule_name: String::new(),
        })
    }
}

/// Remove the firewall rule saved by [`apply_net_policy`].
pub fn restore_net_policy(snapshot: NetPolicySnapshot) -> Result<(), String> {
    if snapshot.rule_name.is_empty() {
        return Ok(());
    }
    let rule = snapshot.rule_name.clone();
    let out = std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name={rule}"),
        ])
        .output()
        .map_err(|e| format!("netsh delete rule spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "netsh delete rule failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    // M-10: drop the rule from the journal (recovered). Ignore errors.
    journal_remove(&rule);
    Ok(())
}

// ─── C2 broker: restricted token + sandboxed spawn ─────────────────────────────
//
// The real Windows sandbox spawn path. Replaces std::process::Command::spawn()
// for sandboxed runs. Creates a restricted token (DISABLE_MAX_PRIVILEGE), spawns
// via CreateProcessAsUserW, and assigns to the C1 Job Object. Also calls C3's
// apply_path_policy before spawn and restore_path_policy after child exit.
//
// Pattern adapted from OpenAI Codex windows-sandbox-rs (token.rs + process.rs).
// Simplified for devboule v1: no AppContainer capabilities, no dedicated sandbox
// user, no private desktop. Just DISABLE_MAX_PRIVILEGE + Job Object + ACLs.

/// A sandboxed child process spawned with a restricted token.
pub struct SandboxedChild {
    process_handle: HANDLE,
    thread_handle: HANDLE,
    pub pid: u32,
    stdout_read: HANDLE,
    stderr_read: HANDLE,
    /// Optional write end of stdin pipe (only set when spawned via spawn_sandboxed_with_stdin).
    stdin_write: HANDLE,
    acl_snapshots: Vec<PathAclSnapshot>,
    restricted_snapshots: Vec<PathSecuritySnapshot>,
    net_snapshot: NetPolicySnapshot,
    job: HANDLE,
    restored: bool, // true after wait_and_restore has restored ACLs + net
}

/// RAII guard that restores filesystem ACLs + net policy when dropped (error path safety).
/// Disarm with `.take()` on success.
struct SandboxGuard {
    acl: Option<Vec<PathAclSnapshot>>,
    net: Option<NetPolicySnapshot>,
}

impl SandboxGuard {
    fn new(acl: Vec<PathAclSnapshot>) -> Self {
        Self {
            acl: Some(acl),
            net: None,
        }
    }
    fn set_net(&mut self, net: NetPolicySnapshot) {
        self.net = Some(net);
    }
    /// Disarm the guard: caller takes ownership of snapshots (success path).
    fn take(mut self) -> (Vec<PathAclSnapshot>, NetPolicySnapshot) {
        let acl = self.acl.take().unwrap_or_default();
        let net = self.net.take().unwrap_or(NetPolicySnapshot {
            rule_name: String::new(),
        });
        (acl, net)
    }
}

impl Drop for SandboxGuard {
    fn drop(&mut self) {
        if let Some(snapshots) = self.acl.take() {
            if !snapshots.is_empty() {
                let _ = restore_path_policy(snapshots);
            }
        }
        if let Some(net) = self.net.take() {
            let _ = restore_net_policy(net);
        }
    }
}

/// RAII guard for restricted-SID ACL mode.
struct RestrictedSandboxGuard {
    restricted: Option<Vec<PathSecuritySnapshot>>,
    net: Option<NetPolicySnapshot>,
}

impl RestrictedSandboxGuard {
    fn new(restricted: Vec<PathSecuritySnapshot>) -> Self {
        Self {
            restricted: Some(restricted),
            net: None,
        }
    }
    fn set_net(&mut self, net: NetPolicySnapshot) {
        self.net = Some(net);
    }
    fn take(mut self) -> (Vec<PathSecuritySnapshot>, NetPolicySnapshot) {
        let restricted = self.restricted.take().unwrap_or_default();
        let net = self.net.take().unwrap_or(NetPolicySnapshot {
            rule_name: String::new(),
        });
        (restricted, net)
    }
}

impl Drop for RestrictedSandboxGuard {
    fn drop(&mut self) {
        if let Some(snapshots) = self.restricted.take() {
            if !snapshots.is_empty() {
                let _ = restore_restricted_sid_policy(snapshots);
            }
        }
        if let Some(net) = self.net.take() {
            let _ = restore_net_policy(net);
        }
    }
}

/// Owns temporary broker handles until the spawn transaction succeeds. This
/// closes every handle on all early-return paths, including failures after a
/// token or pipe has already been created.
struct SpawnHandleCleanup {
    handles: Vec<HANDLE>,
}

impl SpawnHandleCleanup {
    fn new() -> Self {
        Self {
            handles: Vec::new(),
        }
    }

    fn track(&mut self, handle: HANDLE) -> HANDLE {
        self.handles.push(handle);
        handle
    }

    fn close(&mut self, handle: HANDLE) {
        if let Some(index) = self
            .handles
            .iter()
            .position(|candidate| *candidate == handle)
        {
            self.handles.swap_remove(index);
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }

    fn disarm(&mut self, handle: HANDLE) {
        if let Some(index) = self
            .handles
            .iter()
            .position(|candidate| *candidate == handle)
        {
            self.handles.swap_remove(index);
        }
    }
}

impl Drop for SpawnHandleCleanup {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            if !handle.0.is_null() {
                unsafe {
                    let _ = CloseHandle(handle);
                }
            }
        }
    }
}

impl SandboxedChild {
    /// Returns the stdout pipe read handle. Wrap with `std::fs::File::from_raw_handle`.
    pub fn stdout_handle(&self) -> HANDLE {
        self.stdout_read
    }
    /// Returns the stderr pipe read handle.
    pub fn stderr_handle(&self) -> HANDLE {
        self.stderr_read
    }

    /// Take ownership of the stdout pipe handle (for conversion to `File`).
    /// After this call, Drop will NOT close the stdout handle — caller owns it.
    pub fn take_stdout_handle(&mut self) -> HANDLE {
        std::mem::take(&mut self.stdout_read)
    }
    /// Take ownership of the stderr pipe handle (for conversion to `File`).
    pub fn take_stderr_handle(&mut self) -> HANDLE {
        std::mem::take(&mut self.stderr_read)
    }

    /// Take ownership of the stdin pipe write handle (for conversion to `File`).
    /// Only valid when spawned via `spawn_sandboxed_with_stdin`.
    /// After this call, Drop will NOT close the stdin handle — caller owns it.
    pub fn take_stdin_write_handle(&mut self) -> HANDLE {
        std::mem::take(&mut self.stdin_write)
    }

    /// Non-blocking wait. Returns Some(exit_code) if child exited, None if still running.
    pub fn try_wait(&self) -> Result<Option<i32>, String> {
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        let result = unsafe { WaitForSingleObject(self.process_handle, 0) };
        // M-7 fix: distinguish WAIT_OBJECT_0 (0), WAIT_TIMEOUT (258), WAIT_FAILED (0xFFFFFFFF).
        if result.0 == 0 {
            // WAIT_OBJECT_0
            let mut code: u32 = 0;
            unsafe {
                GetExitCodeProcess(self.process_handle, &mut code)
                    .map_err(|e| format!("GetExitCodeProcess failed: {e}"))?;
            }
            Ok(Some(code as i32))
        } else if result.0 == 0xFFFFFFFF {
            // WAIT_FAILED
            Err(format!("WaitForSingleObject failed (WAIT_FAILED)"))
        } else {
            // WAIT_TIMEOUT or other — still running
            Ok(None)
        }
    }

    /// Kill the child process and all its descendants via the Job Object.
    /// If the job is live, uses TerminateJobObject so descendants die immediately.
    /// Does not close the job handle (caller must call wait_and_restore or Drop).
    pub fn kill(&self) -> Result<(), String> {
        unsafe {
            if !self.job.0.is_null() {
                // TerminateJobObject kills all processes in the job, including descendants.
                TerminateJobObject(self.job, 1)
                    .map_err(|e| format!("TerminateJobObject failed: {e}"))?;
            } else {
                // Fallback: no job, just terminate the main process.
                windows::Win32::System::Threading::TerminateProcess(self.process_handle, 1)
                    .map_err(|e| format!("TerminateProcess failed: {e}"))?;
            }
            Ok(())
        }
    }

    /// Wait for the child to exit, then restore the filesystem ACLs (C3).
    /// Handles are closed by `Drop` when the struct goes out of scope.
    pub fn wait_and_restore(&mut self) -> Result<i32, String> {
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        const WAIT_OBJECT_0: u32 = 0;
        const WAIT_TIMEOUT: u32 = 258;
        const RESTORE_WAIT_MS: u32 = 300_000;

        let wait_result = unsafe { WaitForSingleObject(self.process_handle, RESTORE_WAIT_MS) };
        if wait_result.0 == WAIT_TIMEOUT {
            // A caller may invoke wait_and_restore on a hung child. Do not hold
            // the process forever: terminate the complete job before touching
            // host ACL/network state, then wait a bounded second time.
            unsafe {
                if !self.job.0.is_null() {
                    TerminateJobObject(self.job, 1)
                        .map_err(|e| format!("TerminateJobObject after wait timeout failed: {e}"))?;
                } else {
                    windows::Win32::System::Threading::TerminateProcess(self.process_handle, 1)
                        .map_err(|e| format!("TerminateProcess after wait timeout failed: {e}"))?;
                }
            }
            let forced = unsafe { WaitForSingleObject(self.process_handle, 5_000) };
            if forced.0 != WAIT_OBJECT_0 {
                return Err(format!(
                    "sandboxed child did not terminate after timeout (wait result {})",
                    forced.0
                ));
            }
        } else if wait_result.0 != WAIT_OBJECT_0 {
            return Err(format!("WaitForSingleObject failed (result {})", wait_result.0));
        }

        let exit_code = unsafe {
            let mut code: u32 = 0;
            GetExitCodeProcess(self.process_handle, &mut code)
                .map_err(|e| format!("GetExitCodeProcess failed: {e}"))?;
            code as i32
        };

        // C3+C4: descendants must be dead before host ACLs/network policy are
        // restored. Closing the KILL_ON_JOB_CLOSE job enforces that ordering.
        if !self.job.0.is_null() {
            unsafe {
                let job = std::mem::replace(&mut self.job, HANDLE::default());
                let _ = CloseHandle(job);
            }
        }

        // Keep failed restoration state so Drop can retry it rather than
        // silently losing the only snapshots.
        let snapshots = std::mem::take(&mut self.acl_snapshots);
        let restricted = std::mem::take(&mut self.restricted_snapshots);
        let net = std::mem::take(&mut self.net_snapshot);

        let (remaining_snapshots, acl_err) = restore_path_policy_with_remaining(snapshots);
        let (remaining_restricted, restricted_err) =
            restore_restricted_sid_policy_with_remaining(restricted);
        let net_err = restore_net_policy(net.clone()).err();

        if acl_err.is_none() && restricted_err.is_none() && net_err.is_none() {
            self.restored = true;
        } else {
            self.acl_snapshots = remaining_snapshots;
            if restricted_err.is_some() {
                self.restricted_snapshots = remaining_restricted;
            }
            if net_err.is_some() {
                self.net_snapshot = net;
            }
        }

        if let Some(e) = net_err.or(restricted_err).or(acl_err) {
            return Err(e);
        }

        Ok(exit_code)
        // Drop closes ALL handles (child is dead, safe to close job too).
    }
}

// The broker owns kernel handles and all access is serialized by the session
// mutex. HANDLE is an opaque OS resource and is safe to move between threads
// when its close/termination protocol remains under that mutex.
unsafe impl Send for SandboxedChild {}
unsafe impl Sync for SandboxedChild {}

impl Drop for SandboxedChild {
    fn drop(&mut self) {
        unsafe {
            // Close the kill-on-close Job Object FIRST. This terminates the main
            // process and every descendant before any host ACL/network state is
            // restored. The process wait below is only a bounded confirmation.
            if !self.job.0.is_null() {
                let job = std::mem::replace(&mut self.job, HANDLE::default());
                let _ = CloseHandle(job);
            }
            let _ = windows::Win32::System::Threading::WaitForSingleObject(
                self.process_handle,
                5000,
            );

            // NOW restore ACLs + net (the job has been closed).
            if !self.restored {
                let snapshots = std::mem::take(&mut self.acl_snapshots);
                if !snapshots.is_empty() {
                    let _ = restore_path_policy(snapshots);
                }
                let restricted = std::mem::take(&mut self.restricted_snapshots);
                if !restricted.is_empty() {
                    let _ = restore_restricted_sid_policy(restricted);
                }
                let net = std::mem::take(&mut self.net_snapshot);
                let _ = restore_net_policy(net);
            }

            // M-2 fix: check for null before CloseHandle (take_*_handle sets to default).
            if !self.process_handle.0.is_null() {
                let _ = CloseHandle(self.process_handle);
            }
            if !self.thread_handle.0.is_null() {
                let _ = CloseHandle(self.thread_handle);
            }
            if !self.stdout_read.0.is_null() {
                let _ = CloseHandle(self.stdout_read);
            }
            if !self.stderr_read.0.is_null() {
                let _ = CloseHandle(self.stderr_read);
            }
            if !self.stdin_write.0.is_null() {
                let _ = CloseHandle(self.stdin_write);
            }
            // job already closed above.
        }
    }
}

/// Quote one argument using the CommandLineToArgvW/CreateProcess quoting rules.
/// This is required even for absolute paths: Node is commonly installed under
/// `C:\\Program Files\\nodejs`, and an unquoted command line would split it.
fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(ch);
                backslashes = 0;
            }
        }
    }
    // Backslashes immediately before the closing quote must be doubled.
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

fn build_windows_command_line(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(quote_windows_arg)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build a UTF-16 environment block from (KEY, VALUE) pairs.
/// Sorted case-insensitively by key (Windows requires this for CreateProcess).
/// Double-null terminated.
fn make_env_block(env_vars: &[(String, String)]) -> Vec<u16> {
    let mut items: Vec<(String, String, String)> = env_vars
        .iter()
        .map(|(k, v)| (k.to_uppercase(), k.clone(), v.clone()))
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let mut w: Vec<u16> = Vec::new();
    let is_empty = items.is_empty();
    for (_, k, v) in items {
        let entry = format!("{k}={v}");
        w.extend(entry.encode_utf16());
        w.push(0);
    }
    w.push(0); // block terminator
               // M-5 fix: empty block needs two NULs per Windows spec.
    if is_empty {
        w.push(0);
    }
    w
}

/// Create an anonymous pipe whose write end is inheritable by the child.
fn create_pipe() -> Result<(HANDLE, HANDLE), String> {
    use windows::Win32::Foundation::SetHandleInformation;
    use windows::Win32::System::Pipes::CreatePipe;
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe {
        CreatePipe(&mut read, &mut write, None, 0)
            .map_err(|e| format!("CreatePipe failed: {e}"))?;
        SetHandleInformation(write, 0x1u32, windows::Win32::Foundation::HANDLE_FLAGS(0x1))
            .map_err(|e| {
                let _ = CloseHandle(read);
                let _ = CloseHandle(write);
                format!("SetHandleInformation(pipe write) failed: {e}")
            })?;
    }
    Ok((read, write))
}

/// Create the interactive stdin pipe. The child inherits the read end; the
/// parent retains the write end and exposes it through `SandboxedChild`.
fn create_stdin_pipe() -> Result<(HANDLE, HANDLE), String> {
    use windows::Win32::Foundation::SetHandleInformation;
    use windows::Win32::System::Pipes::CreatePipe;
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe {
        CreatePipe(&mut read, &mut write, None, 0)
            .map_err(|e| format!("CreatePipe(stdin) failed: {e}"))?;
        SetHandleInformation(read, 0x1u32, windows::Win32::Foundation::HANDLE_FLAGS(0x1)).map_err(
            |e| {
                let _ = CloseHandle(read);
                let _ = CloseHandle(write);
                format!("SetHandleInformation(stdin read) failed: {e}")
            },
        )?;
        SetHandleInformation(write, 0x1u32, windows::Win32::Foundation::HANDLE_FLAGS(0)).map_err(
            |e| {
                let _ = CloseHandle(read);
                let _ = CloseHandle(write);
                format!("SetHandleInformation(stdin write) failed: {e}")
            },
        )?;
    }
    Ok((read, write))
}

/// Open a handle to the NUL device for use as a child's stdin.
/// The child can read from this without blocking (always returns EOF/0 bytes).
/// The handle is inheritable so the child receives it.
fn open_null_handle() -> Result<HANDLE, String> {
    use windows::Win32::Storage::FileSystem::CreateFileW;
    // Use numeric constants to avoid windows 0.58 import drift:
    // GENERIC_READ=0x80000000, FILE_SHARE_READ|WRITE=0x3,
    // OPEN_EXISTING=3, FILE_ATTRIBUTE_NORMAL=0x80.
    let null_name: Vec<u16> = "NUL\0".encode_utf16().collect();
    unsafe {
        let h = CreateFileW(
            windows::core::PCWSTR(null_name.as_ptr()),
            0x80000000u32,                                             // GENERIC_READ
            windows::Win32::Storage::FileSystem::FILE_SHARE_MODE(0x3), // READ|WRITE
            None,
            windows::Win32::Storage::FileSystem::FILE_CREATION_DISPOSITION(3), // OPEN_EXISTING
            windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0x80), // FILE_ATTRIBUTE_NORMAL
            None,
        )
        .map_err(|e| format!("CreateFileW(NUL) failed: {e}"))?;
        // Make inheritable so the child can use it as stdin.
        // LOW fix: propagate SetHandleInformation failure instead of silently ignoring.
        windows::Win32::Foundation::SetHandleInformation(
            h,
            0x1u32,
            windows::Win32::Foundation::HANDLE_FLAGS(0x1),
        )
        .map_err(|e| {
            let _ = CloseHandle(h);
            format!("SetHandleInformation(NUL) failed: {e}")
        })?;
        Ok(h)
    }
}

/// Open the current process token and create a restricted version
/// with DISABLE_MAX_PRIVILEGE (strips all privileges from the token).
/// Also adds the well-known WinRestrictedCodeSid (S-1-5-12) as a restricted SID,
/// so the second access check evaluates against this SID instead of the user's groups.
fn create_restricted_token() -> Result<HANDLE, String> {
    use windows::Win32::Security::{
        CreateRestrictedToken, CREATE_RESTRICTED_TOKEN_FLAGS, TOKEN_ACCESS_MASK,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut primary_token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ACCESS_MASK(TOKEN_DUPLICATE.0 | TOKEN_QUERY.0 | TOKEN_ASSIGN_PRIMARY.0),
            &mut primary_token,
        )
        .map_err(|e| format!("OpenProcessToken failed: {e}"))?;
    }

    // Create the well-known WinRestrictedCodeSid (S-1-5-12) in a stack buffer.
    // This SID is used for the second access check on securable objects.
    // The buffer must stay alive through the CreateRestrictedToken call.
    let mut restricted_sid: PSID = PSID::default();
    let mut sid_authority = SID_IDENTIFIER_AUTHORITY {
        Value: [0, 0, 0, 0, 0, 5], // SECURITY_NT_AUTHORITY
    };
    unsafe {
        AllocateAndInitializeSid(
            &mut sid_authority,
            1,
            SECURITY_RESTRICTED_CODE_RID, // 12
            0, 0, 0, 0, 0, 0, 0,
            &mut restricted_sid,
        )
        .map_err(|e| format!("AllocateAndInitializeSid(WinRestrictedCodeSid) failed: {e}"))?;
    }

    // Wrap the SID in a SID_AND_ATTRIBUTES array for SidsToRestrict.
    // We use a stack-allocated array to keep the buffer alive through the call.
    let sid_and_attr = [windows::Win32::Security::SID_AND_ATTRIBUTES {
        Sid: restricted_sid,
        // For SidsToRestrict, Windows requires zero or integrity/logon
        // attributes; SE_GROUP_ENABLED is not valid for this parameter.
        Attributes: 0,
    }];

    let mut restricted_token = HANDLE::default();
    unsafe {
        // DISABLE_MAX_PRIVILEGE (0x1) strips privileges;
        // LUA_TOKEN (0x4) produces a filtered token like UAC does.
        // Pass the restricted SID array as SidsToRestrict.
        if let Err(e) = CreateRestrictedToken(
            primary_token,
            CREATE_RESTRICTED_TOKEN_FLAGS(0x1 | 0x4), // DISABLE_MAX_PRIVILEGE | LUA_TOKEN
            None,
            None,
            Some(&sid_and_attr),
            &mut restricted_token,
        ) {
            let _ = CloseHandle(primary_token);
            let _ = FreeSid(restricted_sid);
            return Err(format!("CreateRestrictedToken failed: {e}"));
        }
    }

    unsafe {
        let _ = CloseHandle(primary_token);
        let _ = FreeSid(restricted_sid);
    }
    Ok(restricted_token)
}

/// Create a Job Object with kill-on-close + optional memory limit, CPU time limit, and process count limit.
///
/// When cpu_secs > 0: sets JOB_OBJECT_LIMIT_PROCESS_TIME with PerJobUserTimeLimit in 100ns units.
/// When max_procs > 0: sets JOB_OBJECT_LIMIT_ACTIVE_PROCESS with ActiveProcessLimit.
fn create_job_object(rlimits: &ResourceLimits) -> Result<HANDLE, String> {
    let memory_limit: usize = rlimits
        .addr_space_bytes
        .map(|b| b as usize)
        .unwrap_or(usize::MAX);
    unsafe {
        let job =
            CreateJobObjectW(None, None).map_err(|e| format!("CreateJobObjectW failed: {e}"))?;
        let mut info = std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>();
        let mut basic = std::mem::zeroed::<JOBOBJECT_BASIC_LIMIT_INFORMATION>();
        basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // H-2 fix: enable PROCESS_MEMORY only when a finite limit is set.
        if rlimits.addr_space_bytes.is_some() {
            basic.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        }
        // CPU time limit: JOB_OBJECT_LIMIT_PROCESS_TIME with PerJobUserTimeLimit in 100ns units.
        // Set the limit fields on `basic` FIRST, then copy into info: assigning a zeroed
        // struct over info.BasicLimitInformation afterwards would clobber them (reviewer
        // finding, 2026-07-31 — the fields must not be written after the copy).
        if rlimits.cpu_secs > 0 {
            basic.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
            // cpu_secs is seconds; convert to 100-nanosecond units (1 sec = 10,000,000 100ns units).
            basic.PerJobUserTimeLimit = (rlimits.cpu_secs as i64) * 10_000_000;
        }
        // Active process limit: JOB_OBJECT_LIMIT_ACTIVE_PROCESS with ActiveProcessLimit.
        if rlimits.max_procs > 0 {
            basic.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            basic.ActiveProcessLimit = rlimits.max_procs as u32;
        }
        info.BasicLimitInformation = basic;
        info.ProcessMemoryLimit = memory_limit;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .map_err(|e| format!("SetInformationJobObject failed: {e}"))?;
        Ok(job)
    }
}

/// Spawn a child process with a restricted token, Job Object, and filesystem ACLs.
/// This is the C2 broker: it replaces `std::process::Command::spawn()` for Windows
/// sandboxed runs.
///
/// Integrates C1 (Job Object), C2 (restricted token), and C3 (filesystem ACLs).
/// After the child exits, call `wait_and_restore()` to restore ACLs + close handles.
///
/// Child stdin is connected to NUL (non-blocking, always returns EOF).
pub fn spawn_sandboxed(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    cwd: &Path,
    env_vars: &[(String, String)],
) -> Result<SandboxedChild, String> {
    spawn_sandboxed_internal(policy, program, args, cwd, env_vars, None)
}

/// Spawn a sandboxed child with an OPTIONAL parent-owned stdin pipe.
/// When `provide_stdin_pipe` is true, creates an inheritable pipe for stdin and
/// returns the write end in `SandboxedChild.stdin_write` (caller must take it via
/// `take_stdin_write_handle()`). When false (default), connects child stdin to NUL
/// like `spawn_sandboxed`.
///
/// Preserves CREATE_SUSPENDED + AssignProcessToJobObject before ResumeThread.
/// Child stdin is a pipe ONLY for this path; existing agentic run keeps NUL stdin.
pub fn spawn_sandboxed_with_stdin(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    cwd: &Path,
    env_vars: &[(String, String)],
) -> Result<SandboxedChild, String> {
    spawn_sandboxed_internal(policy, program, args, cwd, env_vars, Some(true))
}

/// Internal implementation shared by both spawn functions.
/// `stdin_pipe`: None = NUL (agentic), Some(true) = pipe (interactive sidecar).
fn spawn_sandboxed_internal(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    cwd: &Path,
    env_vars: &[(String, String)],
    stdin_pipe: Option<bool>,
) -> Result<SandboxedChild, String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
        ResumeThread, UpdateProcThreadAttribute, CREATE_NO_WINDOW, CREATE_SUSPENDED,
        CREATE_UNICODE_ENVIRONMENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_CREATION_FLAGS,
        PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    // C3+C4: apply filesystem ACLs + net policy before spawn.
    // SandboxGuard ensures both are restored even if spawn fails.
    let restricted_snapshots = apply_restricted_sid_policy(policy, cwd, program)?;
    let mut guard = RestrictedSandboxGuard::new(restricted_snapshots);
    guard.set_net(apply_net_policy(policy, program)?);

    let mut cleanup = SpawnHandleCleanup::new();

    // C1: create Job Object.
    let job = cleanup.track(create_job_object(&policy.rlimits)?);

    // C2: create restricted token.
    let restricted_token = cleanup.track(create_restricted_token()?);

    // Create pipes for stdout/stderr.
    let (stdout_read, stdout_write) = create_pipe()?;
    cleanup.track(stdout_read);
    cleanup.track(stdout_write);
    let (stderr_read, stderr_write) = create_pipe()?;
    cleanup.track(stderr_read);
    cleanup.track(stderr_write);

    // Handle stdin: either NUL (agentic) or a pipe (interactive sidecar).
    let (stdin_write, child_stdin) = if stdin_pipe == Some(true) {
        let (read, write) = create_stdin_pipe()?;
        // The child reads from the pipe; the parent keeps the write end.
        (Some(cleanup.track(write)), cleanup.track(read))
    } else {
        let null_stdin = cleanup.track(open_null_handle()?);
        (None, null_stdin)
    };

    // Build a correctly quoted command line. `CreateProcessAsUserW` parses a
    // single mutable command-line buffer rather than receiving argv directly.
    let cmdline = build_windows_command_line(program, args);
    let mut cmdline_wide: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();

    // Build env block.
    let env_block = make_env_block(env_vars);

    // Build cwd wide string.
    let cwd_wide: Vec<u16> = cwd
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // M-4 fix: set lpDesktop to null so the child inherits the parent's desktop.
    // A private window station (CreateDesktopW + ACL grant) is a v2 improvement;
    // the restricted token + LUA_TOKEN already removes admin effectiveness for v1.
    // Keeping null avoids hardcoding a session-specific name.

    // H-9 fix: use STARTUPINFOEXW + PROC_THREAD_ATTRIBUTE_HANDLE_LIST to restrict
    // handle inheritance to ONLY stdout/stderr/stdin. With bInheritHandles=TRUE
    // but a constrained attribute list, the child cannot inherit arbitrary parent
    // handles (e.g. other pipe ends, token handles).
    let inherit_handles: [HANDLE; 3] = [stdout_write, stderr_write, child_stdin];

    // Step 1: query the required attribute-list buffer size. The sizing call
    // normally returns FALSE with ERROR_INSUFFICIENT_BUFFER; any other error
    // is a real setup failure and must not be silently converted to an empty
    // allocation.
    let mut attr_size: usize = 0;
    let sizing_error = unsafe {
        let ok = InitializeProcThreadAttributeList(
            LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
            1,
            0,
            &mut attr_size,
        )
        .is_ok();
        if ok {
            None
        } else {
            let error = windows::Win32::Foundation::GetLastError();
            if error == windows::Win32::Foundation::WIN32_ERROR(122) {
                None
            } else {
                Some(format!(
                    "InitializeProcThreadAttributeList sizing failed: {error:?}"
                ))
            }
        }
    };
    if let Some(error) = sizing_error {
        return Err(error);
    }
    if attr_size == 0 {
        return Err("InitializeProcThreadAttributeList sizing returned zero bytes".to_string());
    }

    // Step 2: allocate the buffer (lives until after CreateProcessAsUserW).
    let mut attr_buf: Vec<u8> = vec![0u8; attr_size];
    let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut std::ffi::c_void);

    // Step 3: initialize + populate the single HANDLE_LIST attribute.
    let handle_list = inherit_handles;
    unsafe {
        let mut initialized_size = attr_size;
        if let Err(e) = InitializeProcThreadAttributeList(attr_list, 1, 0, &mut initialized_size) {
            return Err(format!("InitializeProcThreadAttributeList failed: {e}"));
        }
        if let Err(e) = UpdateProcThreadAttribute(
            attr_list,
            0,
            // PROC_THREAD_ATTRIBUTE_HANDLE_LIST. Do not use the nearby
            // PARENT_PROCESS value (0x00020000): a wrong value silently
            // defeats the inherited-handle containment boundary.
            0x00020002usize,
            Some(handle_list.as_ptr() as *const std::ffi::c_void),
            std::mem::size_of_val(&handle_list),
            None,
            None,
        ) {
            DeleteProcThreadAttributeList(attr_list);
            return Err(format!("UpdateProcThreadAttribute failed: {e}"));
        }
    }

    // STARTUPINFOEXW (superset of STARTUPINFOW, with lpAttributeList).
    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdOutput = stdout_write;
    si.StartupInfo.hStdError = stderr_write;
    si.StartupInfo.hStdInput = child_stdin;
    si.StartupInfo.lpDesktop = windows::core::PWSTR::null();
    si.lpAttributeList = attr_list;

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // H-9: EXTENDED_STARTUPINFO_PRESENT (0x00080000) tells the loader this is a STARTUPINFOEXW.
    let flags = CREATE_SUSPENDED
        | CREATE_NO_WINDOW
        | CREATE_UNICODE_ENVIRONMENT
        | PROCESS_CREATION_FLAGS(0x00080000);

    unsafe {
        if let Err(e) = CreateProcessAsUserW(
            restricted_token,
            PCWSTR::null(),
            PWSTR(cmdline_wide.as_mut_ptr()),
            None,
            None,
            BOOL(1),
            flags,
            Some(env_block.as_ptr() as *const _),
            PCWSTR(cwd_wide.as_ptr()),
            &si as *const _ as *const _,
            &mut pi,
        ) {
            // LOW fix: clean up ALL intermediate handles + attr list before returning.
            // SandboxGuard (Drop) restores ACLs/net.
            DeleteProcThreadAttributeList(attr_list);
            return Err(format!("CreateProcessAsUserW failed: {e}"));
        }
    }

    // The process owns its own copies of the inherited handles now. The
    // parent closes its child-side stdin and pipe write ends.
    unsafe {
        DeleteProcThreadAttributeList(attr_list);
    }
    cleanup.close(stdout_write);
    cleanup.close(stderr_write);
    cleanup.close(child_stdin);
    cleanup.close(restricted_token);
    cleanup.track(pi.hProcess);
    cleanup.track(pi.hThread);

    // C1: assign the SUSPENDED child to the Job Object BEFORE it starts running (C-4 fix).
    // This prevents descendants from escaping before assignment.
    unsafe {
        if let Err(e) = AssignProcessToJobObject(job, pi.hProcess) {
            // H-3 fix: kill the suspended child + close ALL handles + let guard restore ACLs/net.
            cleanup.close(job);
            let _ = windows::Win32::System::Threading::TerminateProcess(pi.hProcess, 1);
            let _ = windows::Win32::System::Threading::WaitForSingleObject(pi.hProcess, 5000);
            return Err(format!("AssignProcessToJobObject failed: {e}"));
        }
    }

    // Resume the child's main thread — it starts executing now, safely inside the Job Object.
    if unsafe { ResumeThread(pi.hThread) } == u32::MAX {
        // The child is already in the kill-on-close job. Close it before
        // waiting so descendants cannot outlive this failed spawn.
        cleanup.close(job);
        unsafe {
            let _ = windows::Win32::System::Threading::TerminateProcess(pi.hProcess, 1);
            let _ = windows::Win32::System::Threading::WaitForSingleObject(pi.hProcess, 5000);
        }
        return Err("ResumeThread failed".to_string());
    }

    // Disarm the guard: on success, snapshots move into SandboxedChild.
    let (restricted_snapshots, net_snapshot) = guard.take();
    cleanup.disarm(pi.hProcess);
    cleanup.disarm(pi.hThread);
    cleanup.disarm(stdout_read);
    cleanup.disarm(stderr_read);
    if let Some(h) = stdin_write {
        cleanup.disarm(h);
    }
    cleanup.disarm(job);

    Ok(SandboxedChild {
        process_handle: pi.hProcess,
        thread_handle: pi.hThread,
        pid: pi.dwProcessId,
        stdout_read,
        stderr_read,
        stdin_write: stdin_write.unwrap_or(HANDLE::default()),
        acl_snapshots: Vec::new(),
        restricted_snapshots,
        net_snapshot,
        job,
        restored: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::sandbox::{ResourceLimits, SandboxPolicy};

    /// True when the current process runs with an elevated (administrator) token.
    /// The ACL layer needs it: `icacls /restore` requires SeRestorePrivilege and
    /// the broker's restricted-SID grant on `C:\Windows` requires SeRestorePrivilege
    /// or owner rights the sandbox cannot assume. Tests that mutate real system
    /// ACLs/firewall rules skip on a non-elevated host instead of failing.
    fn process_is_elevated() -> bool {
        use windows::Win32::Security::TOKEN_ELEVATION;
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        let mut token = HANDLE::default();
        let ok = unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                windows::Win32::Security::TOKEN_QUERY,
                &mut token,
            )
        };
        if ok.is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let res = unsafe {
            windows::Win32::Security::GetTokenInformation(
                token,
                windows::Win32::Security::TokenElevation,
                Some(&mut elevation as *mut _ as *mut _),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
        };
        unsafe {
            let _ = CloseHandle(token);
        }
        res.is_ok() && elevation.TokenIsElevated != 0
    }

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
        // Close the stashed handle so the test does not leak a job across the suite
        // (reviewer finding, 2026-07-31).
        STASHED_JOB.with(|cell| {
            if let Some(h) = cell.borrow_mut().take() {
                unsafe {
                    let _ = CloseHandle(h);
                }
            }
        });
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
        assert!(
            status.success(),
            "cmd /c exit 0 should report success; got {:?}",
            status
        );
    }

    /// Broker integration: create a restricted child with a parent-owned stdin
    /// pipe, then wait for normal exit and restore the broker state. All paths
    /// are confined to a temporary directory, but the broker's restricted-SID
    /// grant on `C:\Windows` (system DLL loading for the S-1-5-12 token) still
    /// requires an elevated shell, so this test skips when the host is not
    /// elevated instead of failing on a normal dev machine.
    #[test]
    fn broker_spawn_with_stdin_exits_and_restores_cleanly() {
        if !process_is_elevated() {
            eprintln!("skipping: restricted-SID grant on C:\\Windows requires elevation");
            return;
        }
        use std::io::{Read, Write};
        use std::os::windows::io::FromRawHandle;

        let temp = std::env::temp_dir().join(format!(
            "devboule_broker_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).expect("create broker test directory");
        let cmd_source = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
            .join("System32")
            .join("cmd.exe");
        let cmd_copy = temp.join("cmd.exe");
        std::fs::copy(&cmd_source, &cmd_copy).expect("copy cmd.exe into test directory");

        let policy = SandboxPolicy::deny(std::path::PathBuf::new())
            .net(crate::backend::sandbox::NetPolicy::Enabled)
            .rlimits(ResourceLimits {
                cpu_secs: 60,
                addr_space_bytes: None,
                max_procs: 16,
            });
        let mut child = spawn_sandboxed_with_stdin(
            &policy,
            &cmd_copy.to_string_lossy(),
            &[
                "/V:ON".to_string(),
                "/C".to_string(),
                "set /p value=& echo !value!".to_string(),
            ],
            &temp,
            &[("SystemRoot".to_string(), "C:\\Windows".to_string())],
        )
        .expect("broker should spawn cmd.exe");

        let stdout = unsafe { std::fs::File::from_raw_handle(child.take_stdout_handle().0) };
        let stderr = unsafe { std::fs::File::from_raw_handle(child.take_stderr_handle().0) };
        let mut stdin =
            unsafe { std::fs::File::from_raw_handle(child.take_stdin_write_handle().0) };
        stdin
            .write_all(b"broker-stdin\n")
            .expect("parent should write to broker stdin");
        stdin.flush().expect("parent should flush broker stdin");

        let exit_code = child
            .wait_and_restore()
            .expect("broker child should exit and restore policy");
        assert_eq!(exit_code, 0);
        drop(stdin);

        let mut stdout_text = String::new();
        let mut stderr_text = String::new();
        let mut stdout = stdout;
        let mut stderr = stderr;
        stdout
            .read_to_string(&mut stdout_text)
            .expect("stdout pipe should remain readable");
        stderr
            .read_to_string(&mut stderr_text)
            .expect("stderr pipe should remain readable");
        assert!(
            stdout_text.contains("broker-stdin"),
            "stdout: {stdout_text:?}"
        );
        assert!(stderr_text.is_empty(), "stderr: {stderr_text:?}");
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// C3: apply deny-write on a temp file, then restore the original ACL.
    /// Verifies the icacls save → deny → restore pipeline works end-to-end.
    /// `icacls /restore` needs SeRestorePrivilege, so this skips on a
    /// non-elevated host.
    #[test]
    fn apply_and_restore_path_policy_roundtrip() {
        if !process_is_elevated() {
            eprintln!("skipping: icacls /restore requires SeRestorePrivilege (elevated shell)");
            return;
        }
        let temp = std::env::temp_dir().join(format!("devboule_c3_test_{}", std::process::id()));
        std::fs::write(&temp, "test").expect("write temp file");

        let policy = SandboxPolicy::deny(temp.clone());
        let snapshots = apply_path_policy(&policy).expect("apply should succeed");
        assert!(!snapshots.is_empty(), "should have at least one snapshot");

        restore_path_policy(snapshots).expect("restore should succeed");

        let _ = std::fs::remove_file(&temp);
    }

    /// C3: apply allow-write on a writable path, then restore.
    /// Uses an existing temp dir as readonly_root (canonicalize_path rejects
    /// nonexistent paths) and a second temp dir as the writable path.
    /// `icacls /restore` needs SeRestorePrivilege (hosts whose %TEMP% ACLs carry
    /// SIDs the caller cannot re-assign fail even on user-owned files), so this
    /// skips on a non-elevated host like `apply_and_restore_path_policy_roundtrip`.
    #[test]
    fn apply_writable_path_and_restore() {
        if !process_is_elevated() {
            eprintln!("skipping: icacls /restore requires SeRestorePrivilege (elevated shell)");
            return;
        }
        let temp =
            std::env::temp_dir().join(format!("devboule_c3_writable_{}", std::process::id()));
        std::fs::create_dir_all(&temp).expect("create temp dir");
        let root = std::env::temp_dir().join(format!("devboule_c3_root_{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create temp root");

        let policy = SandboxPolicy::deny(root).writable(temp.clone());
        let snapshots = apply_path_policy(&policy).expect("apply should succeed");
        assert!(!snapshots.is_empty(), "should have at least one snapshot");

        restore_path_policy(snapshots).expect("restore should succeed");

        let _ = std::fs::remove_dir_all(&temp);
    }
}
