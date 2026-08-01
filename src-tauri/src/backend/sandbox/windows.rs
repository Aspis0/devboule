//! Windows sandbox backend: the C5 AppContainer broker (per-spawn profiles +
//! SECURITY_CAPABILITIES + Job Object + ACL layer) — the full sandbox stack,
//! wired since 2026-07-31 (C6). `is_enforced()` is TRUE on Windows; every
//! app-hosted spawn path routes through `spawn_sandboxed[_with_stdin|_pty]`.
//! The S-1-5-12 restricted-token path (C2) was replaced by AppContainers
//! (package SID + capabilities) and network deny is the `internetClient`
//! capability (kernel-enforced), not netsh.

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
    JOB_OBJECT_LIMIT_JOB_TIME, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};
const SECURITY_RESTRICTED_CODE_RID: u32 = 12;

/// Access mask for writable paths: read+write+execute+DELETE+DELETE_CHILD.
/// DELETE/FILE_DELETE_CHILD are REQUIRED for MoveFileExW(REPLACE_EXISTING)
/// atomic replaces (consent-hook ledger, git/npm workspace writes) — the final
/// hostile review (round 7) proved FILE_GENERIC_WRITE alone lacks them and the
/// hook's ledger replace silently failed. Asserted in tests.
const WRITABLE_ACCESS_MASK: u32 = FILE_GENERIC_READ.0
    | FILE_GENERIC_WRITE.0
    | FILE_GENERIC_EXECUTE.0
    | windows::Win32::Storage::FileSystem::DELETE.0
    | FILE_DELETE_CHILD.0;
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
/// **SUPERSEDED (C5, 2026-07-31): legacy stub — the restricted-token path was
/// REPLACED by per-spawn AppContainer profiles (create_appcontainer_profile +
/// SECURITY_CAPABILITIES via PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES in
/// spawn_sandboxed_internal). Retained as a no-op for pre-broker callers.
/// is_enforced() is TRUE since C6.**
///
/// Historical note: Windows does NOT allow token re-attachment after
/// `CreateProcess`. The `std::process::Command::spawn()` path creates the
/// process without a custom
/// token, so we cannot apply `CreateRestrictedToken` post-spawn.
///
/// The real implementation (per the historical C2 sub-plan — spawning via
/// `CreateProcessAsUserW` with a restricted token) was REPLACED by the C5
/// AppContainer broker; see `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §10.6.
///
/// NOTE (C6, 2026-07-31): superseded by the AppContainer broker (C5) — the
/// S-1-5-12 restricted-token path was REPLACED by per-spawn AppContainer
/// profiles (see `spawn_sandboxed_internal` / `create_appcontainer_profile`).
/// `is_enforced()` is TRUE on Windows since C6; `effective_sandbox_mode()`
/// unlocks Unattended only for broker-gated app-hosted launches (the external
/// conhost path rejects Unattended — see projects.rs
/// `unattended_external_is_rejected`). This legacy entry point is retained as
/// a no-op for callers that predate the broker; do not reintroduce
/// CreateRestrictedToken (package SIDs fail with ERROR_INVALID_PARAMETER
/// there — use SECURITY_CAPABILITIES instead).
pub fn apply_restricted_token(_cmd: &mut std::process::Command) -> Result<(), String> {
    Ok(())
}

// ─── C3: Filesystem ACL layer ────────────────────────────────────────────────
//
// Applies deny-write / allow-write ACLs to paths derived from SandboxPolicy.
// Uses `icacls` CLI (incremental ACE add, preserves existing ACEs — better than
// SDDL replace-all-DACL for v1). Saves the original ACL to a temp file so
// restore_path_policy can put it back after the child exits.
//
// NOTE: the ACL layer is implemented via `SetNamedSecurityInfoW` with
// package-SID ACEs (see apply_restricted_sid_acl); the icacls-based deny/
// grant helpers below are retained for the Everyone-mode path. The
// `Win32_Security_Authorization` feature covers both.
//
// WIRED since C5: `spawn_sandboxed_internal` calls this before spawn and
// `wait_and_restore` restores after child exit. is_enforced() is TRUE since C6.

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
/// **Wired since C5**: `spawn_sandboxed_internal` calls this before spawn;
/// `wait_and_restore` restores after child exit. `is_enforced()` is TRUE
/// since C6.
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

// ─── C3 filesystem ACL layer: package SID (AppContainer) mode ─────────────────
//
// Since C5 the ACL layer grants the AppContainer PACKAGE SID (S-1-15-2-*, per-
// spawn profile) read+execute on explicit read roots and modify on writable
// paths, with inheritance; the double access check evaluates the package SID
// against the child's AppContainer token. Unspecified paths stay inaccessible
// (deny-by-default). The historical S-1-5-12 restricted-SID mode (C2-era) is
// superseded — see apply_restricted_sid_policy.
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
///
/// `package_sid` is the per-spawn AppContainer package SID (C5) — the same SID
/// the broker passed via SECURITY_CAPABILITIES (PROC_THREAD_ATTRIBUTE_
/// SECURITY_CAPABILITIES) when creating the child. Grants target that SID, so
/// only this spawn's child can use the granted paths.
fn apply_restricted_sid_policy(
    policy: &SandboxPolicy,
    cwd: &Path,
    program: &str,
    package_sid: PSID,
) -> Result<Vec<PathSecuritySnapshot>, String> {
    let mut snapshots = Vec::new();
    let mut processed: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();

    // Collect read roots: readonly_root, cwd, the executable parent.
    // Canonicalization failures roll back every ACL already applied in this
    // transaction; they never return through `?` with host ACLs left changed.
    let mut read_roots = Vec::new();
    // C5: paths under SystemRoot (C:\Windows...) are NEVER granted. Stock
    // Windows ships ALL APPLICATION PACKAGES:(RX) on them, so the AppContainer
    // already has read+exec there; writing a package-SID ACE would need
    // elevation and would re-introduce the §10.5 blocker.
    let system_root = std::env::var("SystemRoot")
        .ok()
        .map(PathBuf::from)
        .and_then(|p| std::fs::canonicalize(p).ok());
    let under_system_root = |p: &Path| -> bool {
        if let Some(root) = &system_root {
            p.starts_with(root)
        } else {
            false
        }
    };
    let mut collect_read_root = |candidate: &Path| -> Result<(), String> {
        let canonical = canonicalize_path(candidate)?;
        if under_system_root(&canonical) {
            // Skip: already covered by ALL APPLICATION PACKAGES ACEs. Loud log:
            // if the project itself lives under C:\Windows, this is a silent
            // confinement drop the caller needs to know about.
            eprintln!(
                "[sandbox/windows] skipping ACL grant for system-root path {} (AppContainer                  already reads it via ALL APPLICATION PACKAGES)",
                canonical.display()
            );
            return Ok(());
        }
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
    // C6: per-launch support files (prompt dir, session gitconfig) live outside
    // the project root but the child MUST read them — grant them as read roots
    // too. They are small (1-2 files), so ACL propagation is cheap.
    for extra in &policy.readonly_paths {
        if let Err(e) = collect_read_root(extra) {
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
    // C5: NO user-home grant. AppContainers are deny-by-default, so the child
    // CANNOT read ~/.ssh, ~/.npmrc, ~/.gitconfig unless the policy explicitly
    // lists them (readonly_root / writable_paths). This is stricter than macOS
    // seatbelt's broad reads and removes the .ssh-exposure concern reviewers
    // flagged on 840d142. Tools needing user config must have those paths
    // granted per-project (documented trade-off, spec §10.6).
    // NOTE: granting the whole home is also a performance trap — setting an
    // inheritable ACE on the user's Known Folder triggers a long Windows
    // propagation pass over every file (observed: multi-minute hang).
    // SystemRoot/System32 are NOT granted either (C5): stock Windows ships
    // ALL APPLICATION PACKAGES:(RX) on the system roots, so the AppContainer
    // reads system DLLs with zero ACL writes on this host — this is what
    // removes the §10.5 elevation blocker.

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
            if let Err(e) = apply_restricted_sid_acl(&root, package_sid, GRANT_ACCESS, FILE_GENERIC_READ.0 | FILE_GENERIC_EXECUTE.0, root.is_dir()) {
                let _ = restore_security_descriptor(&root, &sd_backup);
                let _ = restore_restricted_sid_policy(std::mem::take(&mut snapshots));
                return Err(e);
            }
            snapshots.push(PathSecuritySnapshot { path: root, sd_backup });
        }
    }

    // Apply read+write+execute+delete (modify, incl. DELETE/DELETE_CHILD) on
    // writable paths.
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
            // C5: never grant write ACEs under SystemRoot either. Loud log: a
            // writable path under C:\Windows would silently fail writes at
            // runtime (the ACE is dropped here).
            if under_system_root(&canon) {
                eprintln!(
                    "[sandbox/windows] WARNING: writable path {} is under SystemRoot;                      write ACE dropped (AppContainer would need admin to write there)",
                    canon.display()
                );
                continue;
            }
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
                // C6 round 7 (final hostile review): writable MUST include DELETE +
                // FILE_DELETE_CHILD — MoveFileExW(REPLACE_EXISTING) (fs_replace.rs,
                // used by the consent-hook ledger write AND by git/npm atomic
                // replaces in the workspace) fails without them. FILE_GENERIC_WRITE
                // alone lacks DELETE (0x10000) and FILE_DELETE_CHILD (0x40).
                // Parity with macOS seatbelt (allow file-write* includes delete).
                let access = WRITABLE_ACCESS_MASK;
                if let Err(e) = apply_restricted_sid_acl(&canon, package_sid, GRANT_ACCESS, access, canon.is_dir()) {
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
                    package_sid,
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

    // Success: the caller (broker) owns the package SID lifecycle.
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
    // C5: always empty — network blocking is token-capability-based, there is
    // no firewall rule to track. Kept for API shape (restore is a no-op).
    rule_name: String,
}

/// M-10: remove firewall rules left behind by crashed devboule instances.
/// C5: no-op — the netsh rule layer is gone (AppContainer capability SIDs are
/// per-token and die with the token); kept so lib.rs startup call stays valid.
pub fn cleanup_orphaned_firewall_rules() {}

/// Network policy enforcement (C4, C5 rewrite).
///
/// C5: the AppContainer token carries NO network capability unless
/// `NetPolicy::Enabled` (internetClient). With no capability the kernel
/// denies ALL outbound sockets — including raw AFD access — via WFP ALE
/// (Project Zero's analysis). This replaced the netsh advfirewall rule, which
/// required an elevated shell (H-1) and is now gone: network blocking is
/// per-token, dies with the token, needs no journal, no admin.
///
/// The returned snapshot is always empty; retained for API shape.
pub fn apply_net_policy(
    policy: &SandboxPolicy,
    _program: &str,
) -> Result<NetPolicySnapshot, String> {
    use super::NetPolicy;
    match policy.net {
        // C6 round-30: Loopback is implemented per-token via
        // NetworkIsolationSetAppContainerConfig at profile creation
        // (see apply_loopback_exemption); no firewall rule here.
        NetPolicy::None | NetPolicy::Enabled | NetPolicy::Loopback => Ok(NetPolicySnapshot {
            rule_name: String::new(),
        }),
    }
}

/// Remove the firewall rule saved by [`apply_net_policy`].
/// C5: no-op — capability SIDs live in the token and die with it; there is no
/// firewall rule to remove and no journal to update.
pub fn restore_net_policy(_snapshot: NetPolicySnapshot) -> Result<(), String> {
    Ok(())
}

// ─── C2 superseded by C5 AppContainer broker ──────────────────────────────────
//
// The historical C2 plan (restricted token DISABLE_MAX_PRIVILEGE +
// CreateProcessAsUserW, adapted from OpenAI Codex windows-sandbox-rs) was
// REPLACED in C5 by per-spawn AppContainer profiles: package SID via
// SECURITY_CAPABILITIES, net deny-by-default via capability SIDs (not netsh),
// Job Object + ACL layer unchanged. See spawn_sandboxed_internal /
// create_appcontainer_profile. is_enforced() is TRUE since C6.

/// A sandboxed child process: AppContainer token (per-spawn profile, C5) +
/// Job Object + package-SID ACL snapshots. See `spawn_sandboxed_internal`.
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
    /// Spawn sequence number of the AppContainer profile (C5), used to delete
    /// the profile once the child has exited. None on error paths that never
    /// created one.
    appcontainer_seq: Option<u64>,
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
/// RAII: frees a derived AppContainer package SID (PSID) on drop, and deletes
/// the AppContainer profile when the spawn never completed (C5). The manual
/// free_package_sid closure leaked on several early-return paths (reviewer
/// finding 2026-07-31); a guard cannot. When the spawn succeeds the seq is
/// moved into SandboxedChild (whose Drop deletes the profile), and the SID is
/// disarmed here.
struct PackageSidGuard {
    sid: Option<PSID>,
    appcontainer_seq: Option<u64>,
}

impl PackageSidGuard {
    fn new(sid: PSID, appcontainer_seq: u64) -> Self {
        Self {
            sid: Some(sid),
            appcontainer_seq: Some(appcontainer_seq),
        }
    }
    /// Disarm on success: the SID was consumed by CreateProcessAsUserW and the
    /// profile lifecycle moved into SandboxedChild.
    fn disarm(&mut self) {
        self.sid.take();
        self.appcontainer_seq.take();
    }
}

impl Drop for PackageSidGuard {
    fn drop(&mut self) {
        if let Some(sid) = self.sid.take() {
            unsafe {
                let _ = FreeSid(sid);
            }
        }
        // The spawn failed before SandboxedChild existed — do not orphan the
        // profile in %LOCALAPPDATA%\Packages.
        if let Some(seq) = self.appcontainer_seq.take() {
            delete_appcontainer_profile(seq);
        }
    }
}

/// RAII: frees LocalAlloc'd capability SIDs on drop (same leak class).
struct CapabilitySidsGuard {
    sids: Vec<PSID>,
}

impl CapabilitySidsGuard {
    fn new(sids: Vec<PSID>) -> Self {
        Self { sids }
    }
    fn disarm(&mut self) {
        self.sids.clear();
    }
}

impl Drop for CapabilitySidsGuard {
    fn drop(&mut self) {
        for sid in self.sids.drain(..) {
            unsafe {
                let _ = LocalFree(windows::Win32::Foundation::HLOCAL(sid.0 as *mut _));
            }
        }
    }
}

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

        // C5: the child is dead and all host state is restored — delete the
        // per-spawn AppContainer profile so %LOCALAPPDATA%\Packages does not
        // accumulate one entry per spawned command. Best-effort: a leftover
        // profile is inert (only an orphaned empty folder + registry entry).
        if let Some(seq) = self.appcontainer_seq.take() {
            delete_appcontainer_profile(seq);
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

            // C5: delete the per-spawn AppContainer profile on EVERY exit path
            // (Drop runs whether or not wait_and_restore did). wait_and_restore
            // takes the seq, so this only fires when it was never called.
            if let Some(seq) = self.appcontainer_seq.take() {
                delete_appcontainer_profile(seq);
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

/// Create a per-spawn AppContainer profile and return its package SID.
///
/// C5: the MS "Launch an AppContainer" flow requires a REGISTERED profile —
/// a bare derived SID makes CreateProcess* fail with ERROR_FILE_NOT_FOUND
/// (verified 2026-07-31). CreateAppContainerProfile registers it (no admin
/// needed; the profile lands under %LOCALAPPDATA%\Packages). Every spawn gets
/// a unique moniker (pid+seq), so each child has its OWN package SID and ACL
/// grants from concurrent spawns never collide. The profile is deleted in
/// `wait_and_restore` via [`delete_appcontainer_profile`].
/// The caller owns the returned PSID (free with FreeSid); the seq is needed
/// later to delete the profile in `wait_and_restore`.
/// Grant the per-spawn AppContainer package loopback EXEMPTION via
/// `NetworkIsolationSetAppContainerConfig` (C6 round-30 hostile review: the
/// pi sidecar uses `NetPolicy::Loopback` for local Ollama/oMLX sessions, but
/// AppContainers block 127.0.0.1 by default — without this the sidecar could
/// never reach a local model server). Verified callable from a NON-elevated
/// process (HRESULT 0x0 probe). `NetworkIsolationSetAppContainerConfig` is a
/// pure Win32 export in firewallapi — used directly to avoid a windows-crate
/// feature dependency.
fn apply_loopback_exemption(package_sid: PSID) -> Result<(), String> {
    // Resolve dynamically: `firewallapi.lib` is not part of the default
    // linker lib set (LNK1181 without the Windows SDK firewall import lib),
    // but `firewallapi.dll` ships on every Windows 8.1+ and exports the
    // function. Verified callable from a NON-elevated process (HRESULT 0x0).
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn LoadLibraryW(name: *const u16) -> *mut core::ffi::c_void;
            fn GetProcAddress(
                module: *mut core::ffi::c_void,
                name: *const u8,
            ) -> *mut core::ffi::c_void;
        }
        type NetIsolationFn = unsafe extern "system" fn(u32, *const SID_AND_ATTRIBUTES) -> i32;
        let dll: Vec<u16> = "firewallapi.dll"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let name = b"NetworkIsolationSetAppContainerConfig ";
        let module = LoadLibraryW(dll.as_ptr());
        if module.is_null() {
            return Err("LoadLibraryW(firewallapi.dll) failed".to_string());
        }
        let proc = GetProcAddress(module, name.as_ptr());
        if proc.is_null() {
            return Err("GetProcAddress(NetworkIsolationSetAppContainerConfig) failed".to_string());
        }
        let mut sid_and_attrs = SID_AND_ATTRIBUTES {
            Sid: package_sid,
            Attributes: 0,
        };
        let hr = std::mem::transmute::<*mut core::ffi::c_void, NetIsolationFn>(proc)(
            1,
            &mut sid_and_attrs,
        );
        if hr < 0 {
            return Err(format!(
                "NetworkIsolationSetAppContainerConfig failed (HRESULT 0x{:08X});                  loopback exemption not granted — local model sessions (Ollama/oMLX)                  will be unable to reach 127.0.0.1",
                hr as u32
            ));
        }
    }
    Ok(())
}

fn create_appcontainer_profile() -> Result<(PSID, u64), String> {
    use windows::Win32::Security::Isolation::CreateAppContainerProfile;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let moniker = format!("devboule.sandbox.{}.{}", std::process::id(), seq);
    let moniker_wide: Vec<u16> =
        moniker.encode_utf16().chain(std::iter::once(0)).collect();
    let display_wide: Vec<u16> =
        "devboule agent sandbox".encode_utf16().chain(std::iter::once(0)).collect();
    let desc_wide: Vec<u16> =
        "Per-spawn AppContainer for devboule agent commands".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        CreateAppContainerProfile(
            windows::core::PCWSTR(moniker_wide.as_ptr()),
            windows::core::PCWSTR(display_wide.as_ptr()),
            windows::core::PCWSTR(desc_wide.as_ptr()),
            None,
        )
        .map(|sid| (sid, seq))
        .map_err(|e| format!("CreateAppContainerProfile({moniker}) failed: {e}"))
    }
}

/// Delete the AppContainer profile created by [`create_appcontainer_profile`].
/// Called from `wait_and_restore` after the child exited, so profiles do not
/// accumulate in %LOCALAPPDATA%\Packages across spawns. `seq` is the spawn
/// sequence number that was embedded in the moniker at creation time.
fn delete_appcontainer_profile(seq: u64) {
    use windows::Win32::Security::Isolation::DeleteAppContainerProfile;
    let moniker = format!("devboule.sandbox.{}.{}", std::process::id(), seq);
    let moniker_wide: Vec<u16> =
        moniker.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = DeleteAppContainerProfile(windows::core::PCWSTR(moniker_wide.as_ptr()));
    }
}

/// Resolve the AppContainer profile folder (the "AC" dir created by
/// CreateAppContainerProfile) and ensure its Temp subdir exists. The child's
/// LOCALAPPDATA/TEMP/TMP must point here — with an explicit env block, Windows
/// fails the spawn with ERROR_ENVVAR_NOT_FOUND otherwise (verified 2026-07-31,
/// error 0x800700CB).
fn appcontainer_env_paths(package_sid: PSID) -> Result<(String, String), String> {
    use windows::Win32::Security::Isolation::GetAppContainerFolderPath;
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Foundation::LocalFree;
    unsafe {
        // GetAppContainerFolderPath takes the SID as a string, not a PSID.
        let mut sid_string = windows::core::PWSTR::null();
        ConvertSidToStringSidW(package_sid, &mut sid_string)
            .map_err(|e| format!("ConvertSidToStringSidW failed: {e}"))?;
        let sid_str = sid_string.to_string().unwrap_or_default();
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(sid_string.as_ptr() as *mut _));
        let folder_wide: Vec<u16> =
            sid_str.encode_utf16().chain(std::iter::once(0)).collect();
        let folder = GetAppContainerFolderPath(windows::core::PCWSTR(folder_wide.as_ptr()))
            .map_err(|e| format!("GetAppContainerFolderPath failed: {e}"))?;
        let folder_str = folder.to_string().unwrap_or_default();
        // Free the returned PWSTR (LocalAlloc per MS docs).
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(folder.as_ptr() as *mut _));
        if folder_str.is_empty() {
            return Err("GetAppContainerFolderPath returned empty path".to_string());
        }
        let temp_dir = format!(r"{}\Temp", folder_str);
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| format!("create AppContainer Temp dir {temp_dir}: {e}"))?;
        Ok((folder_str, temp_dir))
    }
}

/// Open the current process's primary token (TOKEN_ASSIGN_PRIMARY) for
/// CreateProcessAsUserW. Windows derives the AppContainer token from this one
/// plus SECURITY_CAPABILITIES.
fn open_process_token() -> Result<HANDLE, String> {
    use windows::Win32::Security::TOKEN_ACCESS_MASK;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ACCESS_MASK(
                windows::Win32::Security::TOKEN_DUPLICATE.0
                    | windows::Win32::Security::TOKEN_QUERY.0
                    | windows::Win32::Security::TOKEN_ASSIGN_PRIMARY.0,
            ),
            &mut token,
        )
        .map_err(|e| format!("OpenProcessToken failed: {e}"))?;
    }
    Ok(token)
}

/// Build the capability SID array for the AppContainer's SECURITY_CAPABILITIES.
/// `NetPolicy::Enabled` adds internetClient (SE_GROUP_ENABLED, per the MS
/// "Launch an AppContainer" sample); `None` adds nothing — kernel-enforced
/// deny-by-default network. The returned Vec owns LocalAlloc'd PSIDs; free
/// each with LocalFree after CreateProcessAsUserW.
fn build_capability_sids(policy: &SandboxPolicy) -> Result<Vec<SID_AND_ATTRIBUTES>, String> {
    use windows::Win32::Security::DeriveCapabilitySidsFromName;
    use windows::Win32::System::SystemServices::SE_GROUP_ENABLED;
    if policy.net != crate::backend::sandbox::NetPolicy::Enabled {
        return Ok(Vec::new());
    }
    let mut cap_groups: *mut PSID = std::ptr::null_mut();
    let mut cap_group_count: u32 = 0;
    let mut cap_sids_ptr: *mut PSID = std::ptr::null_mut();
    let mut cap_sid_count: u32 = 0;
    let cap_name: Vec<u16> = "internetClient".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        DeriveCapabilitySidsFromName(
            windows::core::PCWSTR(cap_name.as_ptr()),
            &mut cap_groups,
            &mut cap_group_count,
            &mut cap_sids_ptr,
            &mut cap_sid_count,
        )
        .map_err(|e| format!("DeriveCapabilitySidsFromName(internetClient) failed: {e}"))?;
    }
    let mut caps = Vec::new();
    unsafe {
        for i in 0..cap_sid_count {
            let sid = *cap_sids_ptr.add(i as usize);
            caps.push(SID_AND_ATTRIBUTES {
                Sid: sid,
                Attributes: SE_GROUP_ENABLED as u32,
            });
        }
        // Group SIDs are only used for services; the MS sample frees them.
        for i in 0..cap_group_count {
            let g = *cap_groups.add(i as usize);
            let _ = LocalFree(windows::Win32::Foundation::HLOCAL(g.0 as *mut _));
        }
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(cap_groups as *mut _));
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(cap_sids_ptr as *mut _));
    }
    Ok(caps)
}

/// Create a Job Object with kill-on-close + optional memory limit, CPU time limit, and process count limit.
///
/// When cpu_secs > 0: sets JOB_OBJECT_LIMIT_JOB_TIME with PerJobUserTimeLimit in 100ns units.
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
        // CPU time limit: JOB_OBJECT_LIMIT_JOB_TIME with PerJobUserTimeLimit in 100ns units.
        // (NOT JOB_OBJECT_LIMIT_PROCESS_TIME — that flag pairs with
        // PerProcessUserTimeLimit and SetInformationJobObject rejects the
        // mismatch with ERROR_INVALID_PARAMETER, verified 2026-07-31.)
        // Set the limit fields on `basic` FIRST, then copy into info: assigning a zeroed
        // struct over info.BasicLimitInformation afterwards would clobber them (reviewer
        // finding, 2026-07-31 — the fields must not be written after the copy).
        if rlimits.cpu_secs > 0 {
            basic.LimitFlags |= JOB_OBJECT_LIMIT_JOB_TIME;
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

/// Spawn a child process with an AppContainer token (C5), Job Object, and
/// filesystem ACLs. This is the Windows sandbox broker: it replaces
/// `std::process::Command::spawn()` for Windows sandboxed runs.
///
/// Integrates the AppContainer profile (per-spawn package SID +
/// SECURITY_CAPABILITIES net capability), Job Object (C1), and the
/// filesystem ACL layer (C3). After the child exits, call `wait_and_restore()`
/// to restore ACLs + delete the profile.
///
/// Child stdin is connected to NUL (non-blocking, always returns EOF).
pub fn spawn_sandboxed(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    cwd: &Path,
    env_vars: &[(String, String)],
) -> Result<SandboxedChild, String> {
    spawn_sandboxed_internal(policy, program, args, cwd, env_vars, StdioMode::Pipes, None)
}

/// Spawn a sandboxed child whose stdio is a ConPTY (interactive agent terminal).
/// `hpc` is the pseudoconsole handle (HPCON) created by the caller; the broker
/// passes it via PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE. The returned
/// SandboxedChild has NO pipe handles — the master side (read/write/resize)
/// lives in the caller's ConPTY handles. All sandbox layers (AppContainer,
/// Job Object, ACLs, net capability) apply identically.
pub fn spawn_sandboxed_pty(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    cwd: &Path,
    env_vars: &[(String, String)],
    hpc: windows::Win32::System::Console::HPCON,
) -> Result<SandboxedChild, String> {
    spawn_sandboxed_internal(policy, program, args, cwd, env_vars, StdioMode::ConPty(hpc), None)
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
    spawn_sandboxed_internal(policy, program, args, cwd, env_vars, StdioMode::Pipes, Some(true))
}

/// Internal implementation shared by both spawn functions.
/// `stdin_pipe`: None = NUL (agentic), Some(true) = pipe (interactive sidecar).
/// How the sandboxed child's stdio is wired (C6).
pub enum StdioMode {
    /// Standard pipes: NUL stdin (agentic runs) or a parent-owned stdin pipe
    /// (interactive sidecars). stdout/stderr are always pipes.
    Pipes,
    /// ConPTY (HPCON): the child's stdio is the pseudoconsole. Used by the
    /// interactive agent terminal path (agent_pty.rs) so agents get a real
    /// console (keystrokes, resize, DSR) inside the AppContainer.
    ConPty(windows::Win32::System::Console::HPCON),
}

fn spawn_sandboxed_internal(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    cwd: &Path,
    env_vars: &[(String, String)],
    stdio: StdioMode,
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

    // C5: derive the per-spawn AppContainer package SID once — the SAME SID
    // goes into the token's SidsToRestrict AND the filesystem ACL grants, so
    // only this spawn's child can use the granted paths. Freed after the
    // token+ACL transaction (both copy the SID into their own structures).
    let (package_sid, appcontainer_seq) = match create_appcontainer_profile() {
        Ok(pair) => pair,
        Err(e) => return Err(e),
    };
    let mut package_sid_guard = PackageSidGuard::new(package_sid, appcontainer_seq);

    // C6 round-30: NetPolicy::Loopback needs the per-package loopback
    // EXEMPTION (AppContainers block 127.0.0.1 by default). The pi sidecar
    // uses Loopback for local Ollama/oMLX sessions. Applied right after the
    // profile is registered, before the token/ACL transaction. Fail-closed:
    // an exemption failure aborts the spawn (a local session that cannot
    // reach its model is a silent hang otherwise).
    if policy.net == crate::backend::sandbox::NetPolicy::Loopback {
        apply_loopback_exemption(package_sid)?;
    }

    // C5 oracle sentinel (2026-07-31): the AppContainer child inherits the
    // caller's token identity — kernel-enforced Low IL + package-SID filtering
    // bound it, but an ELEVATED devboule would hand the child an elevated
    // identity. The plan requires unprivileged operation (tauri#13926); log
    // loudly if that invariant is ever violated so it cannot go unnoticed.
    if process_is_elevated() {
        eprintln!(
            "[sandbox/windows] WARNING: devboule is running ELEVATED; AppContainer              children inherit the elevated identity. Run devboule unprivileged              (spec invariant, tauri#13926)."
        );
    }

    // C3+C4: apply filesystem ACLs + net policy before spawn.
    // SandboxGuard ensures both are restored even if spawn fails.
    let restricted_snapshots = match apply_restricted_sid_policy(policy, cwd, program, package_sid) {
        Ok(snaps) => snaps,
        Err(e) => {
            return Err(e);
        }
    };
    let mut guard = RestrictedSandboxGuard::new(restricted_snapshots);
    match apply_net_policy(policy, program) {
        Ok(net) => guard.set_net(net),
        Err(e) => {
            return Err(e);
        }
    }

    let mut cleanup = SpawnHandleCleanup::new();

    // C1: create Job Object.
    let job = match create_job_object(&policy.rlimits) {
        Ok(j) => cleanup.track(j),
        Err(e) => {
            return Err(e);
        }
    };

    // C2: create the AppContainer restricted token (package SID + optional
    // internetClient capability; net enforcement is kernel-side, no firewall).
    // C2 (C5): per the MS "Launch an AppContainer" pattern, we do NOT build a
    // restricted token ourselves. We pass the caller's primary token to
    // CreateProcessAsUserW and hand Windows a SECURITY_CAPABILITIES
    // (package SID + capability SIDs) via PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES;
    // Windows derives the AppContainer token at process creation.
    let primary_token = match open_process_token() {
        Ok(t) => cleanup.track(t),
        Err(e) => {
            return Err(e);
        }
    };
    // Capability SIDs (internetClient only when Enabled). Freed via guard.
    let caps = match build_capability_sids(policy) {
        Ok(c) => c,
        Err(e) => return Err(e),
    };
    let mut caps_guard = CapabilitySidsGuard::new(caps.iter().map(|c| c.Sid).collect());
    // NOTE: package_sid is NOT freed here — SECURITY_CAPABILITIES (built later)
    // references it by pointer and UpdateProcThreadAttribute only shallow-copies
    // the struct. Freed after CreateProcessAsUserW succeeds (or on each error
    // path after this point).

    // C6: ConPTY mode wires the pseudoconsole as the child's stdio; the pipes
    // below exist only in Pipes mode. In ConPty mode the SandboxedChild carries
    // no pipe handles (the caller owns the ConPTY master side).
    let conpty_handle: Option<windows::Win32::System::Console::HPCON> = match stdio {
        StdioMode::ConPty(hpc) => Some(hpc),
        StdioMode::Pipes => None,
    };

    // Create pipes for stdout/stderr.
    let (stdout_read, stdout_write) = create_pipe()?;
    cleanup.track(stdout_read);
    cleanup.track(stdout_write);
    let (stderr_read, stderr_write) = create_pipe()?;
    cleanup.track(stderr_read);
    cleanup.track(stderr_write);

    // Handle stdin: either NUL (agentic) or a pipe (interactive sidecar).
    // Unused in ConPty mode (the pseudoconsole owns stdio).
    let (stdin_write, child_stdin) = if conpty_handle.is_some() {
        (None, HANDLE::default())
    } else if stdin_pipe == Some(true) {
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
    // C5: AppContainers REQUIRE LOCALAPPDATA/TEMP/TMP pointing at the profile
    // AC folder (spawn fails with ERROR_ENVVAR_NOT_FOUND otherwise). Override
    // whatever the caller passed — these MUST be the container's own paths.
    let (localappdata, temp_dir) = match appcontainer_env_paths(package_sid) {
        Ok(p) => p,
        Err(e) => {
            return Err(e);
        }
    };
    let mut broker_env: Vec<(String, String)> = env_vars.to_vec();
    broker_env.retain(|(k, _)| {
        !k.eq_ignore_ascii_case("LOCALAPPDATA")
            && !k.eq_ignore_ascii_case("TEMP")
            && !k.eq_ignore_ascii_case("TMP")
    });
    broker_env.push(("LOCALAPPDATA".to_string(), localappdata));
    broker_env.push(("TEMP".to_string(), temp_dir.clone()));
    broker_env.push(("TMP".to_string(), temp_dir));
    let env_block = make_env_block(&broker_env);

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
    // C5: SECURITY_CAPABILITIES makes Windows create an AppContainer token.
    // C6: in ConPty mode, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE wires the
    // pseudoconsole; stdio handles are INVALID_HANDLE_VALUE per portable-pty.
    let conpty_mode = conpty_handle.is_some();
    let inherit_handles: [HANDLE; 3] = [stdout_write, stderr_write, child_stdin];
    let attr_count: usize = if conpty_mode { 3 } else { 2 };

    // SECURITY_CAPABILITIES: package SID + capability SIDs. Must stay alive
    // through CreateProcessAsUserW (the attribute list copies the struct, but
    // the SIDs are referenced, not copied).
    let mut security_capabilities = windows::Win32::Security::SECURITY_CAPABILITIES {
        AppContainerSid: package_sid,
        // Explicit null when there are no capabilities (an empty Vec's dangling
        // as_ptr would be UB-ish; count==0 makes the kernel ignore it, but pass
        // null per the MS sample).
        Capabilities: if caps.is_empty() {
            std::ptr::null_mut()
        } else {
            caps.as_ptr() as *mut _
        },
        CapabilityCount: caps.len() as u32,
        Reserved: 0,
    };

    // Step 1: query the required attribute-list buffer size. The sizing call
    // normally returns FALSE with ERROR_INSUFFICIENT_BUFFER; any other error
    // is a real setup failure and must not be silently converted to an empty
    // allocation.
    let mut attr_size: usize = 0;
    let sizing_error = unsafe {
        let ok = InitializeProcThreadAttributeList(
            LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
            attr_count as u32,
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

    // Step 3: initialize + populate the HANDLE_LIST and SECURITY_CAPABILITIES
    // attributes.
    let handle_list = inherit_handles;
    unsafe {
        let mut initialized_size = attr_size;
        if let Err(e) = InitializeProcThreadAttributeList(
            attr_list,
            attr_count as u32,
            0,
            &mut initialized_size,
        ) {
            return Err(format!("InitializeProcThreadAttributeList failed: {e}"));
        }
        // In ConPty mode the pseudoconsole replaces the handle list: the child
        // inherits NO handles and stdio comes from the HPCON.
        if !conpty_mode {
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
        } else {
            // C6: PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE (0x20016). The HPCON must
            // stay alive through CreateProcessAsUserW (referenced, not copied);
            // the caller closes it after the child is reaped.
            let hpc = conpty_handle.expect("conpty mode implies handle");
            if let Err(e) = UpdateProcThreadAttribute(
                attr_list,
                0,
                0x00020016usize,
                // NOTE: PSEUDOCONSOLE takes the HPCON VALUE itself as lpValue
                // (like a scalar), NOT a pointer to it — unlike
                // SECURITY_CAPABILITIES which is passed by address. portable-pty
                // does the same (`UpdateProcThreadAttribute(..., con, ...)`).
                Some(hpc.0 as *const std::ffi::c_void),
                std::mem::size_of::<windows::Win32::System::Console::HPCON>() as usize,
                None,
                None,
            ) {
                DeleteProcThreadAttributeList(attr_list);
                return Err(format!(
                    "UpdateProcThreadAttribute(PSEUDOCONSOLE) failed: {e}"
                ));
            }
        }
        // C5: SECURITY_CAPABILITIES (0x00020009) — the AppContainer identity.
        if let Err(e) = UpdateProcThreadAttribute(
            attr_list,
            0,
            // PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES
            0x00020009usize,
            Some((&mut security_capabilities as *mut _) as *const std::ffi::c_void),
            std::mem::size_of::<windows::Win32::Security::SECURITY_CAPABILITIES>() as usize,
            None,
            None,
        ) {
            DeleteProcThreadAttributeList(attr_list);
            return Err(format!(
                "UpdateProcThreadAttribute(SECURITY_CAPABILITIES) failed: {e}"
            ));
        }
    }

    // STARTUPINFOEXW (superset of STARTUPINFOW, with lpAttributeList).
    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    // C6: ConPty mode sets stdio handles to INVALID_HANDLE_VALUE (portable-pty
    // pattern) so the child never inherits the parent's redirected stdio.
    if conpty_mode {
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdOutput = HANDLE(u32::MAX as *mut _);
        si.StartupInfo.hStdError = HANDLE(u32::MAX as *mut _);
        si.StartupInfo.hStdInput = HANDLE(u32::MAX as *mut _);
    } else {
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdOutput = stdout_write;
        si.StartupInfo.hStdError = stderr_write;
        si.StartupInfo.hStdInput = child_stdin;
    }
    si.StartupInfo.lpDesktop = windows::core::PWSTR::null();
    si.lpAttributeList = attr_list;

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // H-9: EXTENDED_STARTUPINFO_PRESENT (0x00080000) tells the loader this is a STARTUPINFOEXW.
    // C6: CREATE_NO_WINDOW is NOT set in ConPty mode — it breaks ConPTY output
    // (verified 2026-07-31: with it, the child never renders into the
    // pseudoconsole and the master reads nothing; without it, the ConPTY init
    // sequences + child output flow normally). In Pipes mode it stays, keeping
    // agentic/one-shot runs console-free.
    let mut flags = CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | PROCESS_CREATION_FLAGS(0x00080000);
    if !conpty_mode {
        flags |= CREATE_NO_WINDOW;
    }

    unsafe {
        if let Err(e) = CreateProcessAsUserW(
            primary_token,
            PCWSTR::null(),
            PWSTR(cmdline_wide.as_mut_ptr()),
            None,
            None,
            // C6: in ConPty mode there is no HANDLE_LIST — the pseudoconsole is
            // wired via attribute. bInheritHandles must be FALSE or the child
            // inherits EVERY parent handle (and the ConPTY host setup can stall).
            // In Pipes mode the HANDLE_LIST restricts inheritance, so TRUE + the
            // attribute list is the safe combo there.
            BOOL(if conpty_mode { 0 } else { 1 }),
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
    // C6: in ConPty mode the child's stdio is the pseudoconsole — the pipes we
    // created above were never handed to it. Close the read ends so no handle
    // is leaked and the child carries default handles.
    let (stdout_read, stderr_read, stdin_write) = if conpty_mode {
        cleanup.close(stdout_read);
        cleanup.close(stderr_read);
        if let Some(w) = stdin_write {
            cleanup.close(w);
        }
        (HANDLE::default(), HANDLE::default(), None)
    } else {
        (stdout_read, stderr_read, stdin_write)
    };
    cleanup.close(primary_token);
    // C5: the child token exists; package SID + capability SIDs are no longer
    // referenced by SECURITY_CAPABILITIES. Disarm both guards.
    package_sid_guard.disarm();
    caps_guard.disarm();
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
        appcontainer_seq: Some(appcontainer_seq),
        restored: false,
    })
}

/// True when the current process runs with an elevated (administrator) token.
/// Used by the broker as a sentinel (C5 oracle finding: an elevated parent
/// hands the AppContainer child an elevated identity) and by tests that mutate
/// real system ACLs/firewall rules, which skip on a non-elevated host.
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

// ─── C6: ConPTY master + portable_pty trait impls ─────────────────────────────
//
// The interactive agent terminal (agent_pty.rs) needs a real console inside the
// AppContainer. portable_pty's own ConPTY spawn cannot be pointed at our broker,
// so we create the pseudoconsole here and expose it through portable_pty's
// traits: SandboxedChild becomes a portable_pty::Child, and WindowsConPtyMaster
// implements MasterPty over a caller-created HPCON + the two pipes.

/// Create a ConPTY (HPCON) plus the two pipes that form its host side.
/// Returns (master, hpc):
/// - master: read/write/resize endpoints for the app (agent_pty uses them).
/// - hpc: the pseudoconsole handle to pass to [`spawn_sandboxed_pty`]; the
///   caller must keep it alive until the child is reaped (it is referenced by
///   the child's stdio, not copied), then close it with ClosePseudoConsole.
pub fn create_conpty(
    rows: u16,
    cols: u16,
) -> Result<(WindowsConPtyMaster, windows::Win32::System::Console::HPCON), String> {
    use windows::Win32::System::Console::{
        CreatePseudoConsole, COORD, HPCON,
    };
    // Same pipe wiring as portable-pty's PsuedoCon::new: hInput = read end of
    // the host→child pipe, hOutput = write end of the child→host pipe.
    let (input_read, input_write) = create_pipe()?;
    let (output_read, output_write) = create_pipe()?;
    let hpc = unsafe {
        CreatePseudoConsole(
            COORD { X: cols as i16, Y: rows as i16 },
            input_read,
            output_write,
            0,
        )
        .map_err(|e| format!("CreatePseudoConsole failed: {e}"))?
    };
    let master = WindowsConPtyMaster {
        hpc,
        input_write,
        output_read,
        // The read end of the host pipe and write end of the child pipe are
        // owned by the OS now; close our copies on drop.
        _input_read: input_read,
        _output_write: output_write,
        rows,
        cols,
    };
    Ok((master, hpc))
}

/// Host side of a ConPTY: writing to `input_write` feeds the child's stdin,
/// reading from `output_read` consumes the child's stdout/stderr, and resize
/// resizes the pseudoconsole. Implements portable_pty::MasterPty so
/// agent_pty.rs can keep its trait-based plumbing.
pub struct WindowsConPtyMaster {
    hpc: windows::Win32::System::Console::HPCON,
    input_write: HANDLE,
    output_read: HANDLE,
    _input_read: HANDLE,
    _output_write: HANDLE,
    rows: u16,
    cols: u16,
}

impl Drop for WindowsConPtyMaster {
    fn drop(&mut self) {
        unsafe {
            // ClosePseudoConsole is async: it signals the conhost to exit but
            // does not block. portable-pty calls it in PsuedoCon::drop too.
            let _ = windows::Win32::System::Console::ClosePseudoConsole(self.hpc);
            let _ = CloseHandle(self.input_write);
            let _ = CloseHandle(self.output_read);
            let _ = CloseHandle(self._input_read);
            let _ = CloseHandle(self._output_write);
        }
    }
}

impl WindowsConPtyMaster {
    /// Duplicate `output_read` for the reader thread (each reader gets its own
    /// handle; the master keeps the original for its lifetime).
    pub fn duplicate_reader(&self) -> Result<HANDLE, String> {
        unsafe {
            let mut dup = HANDLE::default();
            windows::Win32::Foundation::DuplicateHandle(
                windows::Win32::System::Threading::GetCurrentProcess(),
                self.output_read,
                windows::Win32::System::Threading::GetCurrentProcess(),
                &mut dup,
                0,
                false,
                windows::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            )
            .map_err(|e| format!("DuplicateHandle(reader) failed: {e}"))?;
            Ok(dup)
        }
    }

    /// Duplicate `input_write` for the writer (the master keeps the original).
    pub fn duplicate_writer(&self) -> Result<HANDLE, String> {
        unsafe {
            let mut dup = HANDLE::default();
            windows::Win32::Foundation::DuplicateHandle(
                windows::Win32::System::Threading::GetCurrentProcess(),
                self.input_write,
                windows::Win32::System::Threading::GetCurrentProcess(),
                &mut dup,
                0,
                false,
                windows::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            )
            .map_err(|e| format!("DuplicateHandle(writer) failed: {e}"))?;
            Ok(dup)
        }
    }
}

// SAFETY: the master owns raw handles; all access is through &self methods that
// are internally synchronized by the OS (pipe I/O is thread-safe per handle),
// and each duplicated handle is owned by exactly one thread. Matches the
// existing unsafe impl for SandboxedChild.
unsafe impl Send for WindowsConPtyMaster {}
unsafe impl Sync for WindowsConPtyMaster {}

impl std::fmt::Debug for WindowsConPtyMaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsConPtyMaster")
            .field("hpc", &self.hpc.0)
            .field("rows", &self.rows)
            .field("cols", &self.cols)
            .finish()
    }
}

impl portable_pty::MasterPty for WindowsConPtyMaster {
    fn resize(&self, size: portable_pty::PtySize) -> anyhow::Result<()> {
        use windows::Win32::System::Console::ResizePseudoConsole;
        unsafe {
            ResizePseudoConsole(
                self.hpc,
                windows::Win32::System::Console::COORD {
                    X: size.cols as i16,
                    Y: size.rows as i16,
                },
            )
            .map_err(|e| {
                anyhow::anyhow!("ResizePseudoConsole failed: {e}")
            })
        }
    }

    fn get_size(&self) -> anyhow::Result<portable_pty::PtySize> {
        Ok(portable_pty::PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    }

    fn try_clone_reader(
        &self,
    ) -> anyhow::Result<Box<dyn std::io::Read + Send>> {
        use std::os::windows::io::FromRawHandle;
        let dup = self
            .duplicate_reader()
            .map_err(|e| anyhow::anyhow!(e))?;
        // Safety: dup is a fresh handle owned by us.
        let file = unsafe { std::fs::File::from_raw_handle(dup.0 as _) };
        Ok(Box::new(file))
    }

    fn take_writer(&self) -> anyhow::Result<Box<dyn std::io::Write + Send>> {
        use std::os::windows::io::FromRawHandle;
        let dup = self
            .duplicate_writer()
            .map_err(|e| anyhow::anyhow!(e))?;
        // Safety: dup is a fresh handle owned by us.
        let file = unsafe { std::fs::File::from_raw_handle(dup.0 as _) };
        Ok(Box::new(file))
    }
}

/// SandboxedChild as a portable_pty::Child. `wait` maps to wait_and_restore
/// (child reaped + ACLs/AppContainer profile restored); `kill` terminates the
/// whole job (descendants included).
impl std::fmt::Debug for SandboxedChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxedChild")
            .field("pid", &self.pid)
            .field("restored", &self.restored)
            .finish()
    }
}

impl portable_pty::ChildKiller for SandboxedChild {
    fn kill(&mut self) -> std::io::Result<()> {
        SandboxedChild::kill(self).map_err(std::io::Error::other)
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(SandboxedChildKiller {
            pid: self.pid,
            process_handle: self.process_handle,
            job: self.job,
        })
    }
}

impl portable_pty::Child for SandboxedChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        SandboxedChild::try_wait(self)
            .map(|opt| opt.map(|code| portable_pty::ExitStatus::with_exit_code(code as u32)))
            .map_err(std::io::Error::other)
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        // wait_and_restore reaps the child AND restores ACLs + AppContainer
        // profile — the PTY teardown path relies on this.
        let code = self.wait_and_restore().map_err(std::io::Error::other)?;
        Ok(portable_pty::ExitStatus::with_exit_code(code as u32))
    }

    fn process_id(&self) -> Option<u32> {
        Some(self.pid)
    }

    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        Some(self.process_handle.0 as _)
    }
}

/// Standalone killer for a sandboxed child (what clone_killer returns): holds
/// the raw handles WITHOUT the ACL/profile restore responsibility, so it can
/// be shared across threads (Send+Sync) and used after the child object moved
/// into the reap path. Terminating the job kills all descendants.
#[derive(Clone)]
struct SandboxedChildKiller {
    pid: u32,
    process_handle: HANDLE,
    job: HANDLE,
}

impl std::fmt::Debug for SandboxedChildKiller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxedChildKiller")
            .field("pid", &self.pid)
            .finish()
    }
}

// SAFETY: raw handles are only used for TerminateJobObject/TerminateProcess
// which are thread-safe; the killer is Clone + shared across reader threads.
unsafe impl Send for SandboxedChildKiller {}
unsafe impl Sync for SandboxedChildKiller {}

impl portable_pty::ChildKiller for SandboxedChildKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        unsafe {
            if !self.job.0.is_null() {
                TerminateJobObject(self.job, 1).map_err(std::io::Error::other)?;
            } else if !self.process_handle.0.is_null() {
                windows::Win32::System::Threading::TerminateProcess(self.process_handle, 1)
                    .map_err(std::io::Error::other)?;
            }
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

mod tests {
    use super::*;
    use crate::backend::sandbox::{NetPolicy, ResourceLimits, SandboxPolicy};

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

    /// Broker integration: create an AppContainer child with a parent-owned
    /// stdin pipe, then wait for normal exit and restore the broker state.
    /// C5: the package-SID grants now cover only user paths (cwd, exe parent,
    /// home read-only) and system DLL access comes from the stock
    /// ALL APPLICATION PACKAGES ACEs — NO C:\Windows ACL writes, so this test
    /// runs on a non-elevated shell (the §10.5 blocker is gone).
    #[test]
    fn broker_spawn_with_stdin_exits_and_restores_cleanly() {
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

        // C5 assertion: the child must be running as an AppContainer — verify
        // TokenIsAppContainer on its process token (the load-bearing property
        // of this milestone: kernel-enforced net deny + package-SID ACLs).
        {
            use windows::Win32::Security::TokenIsAppContainer;
            use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
            let proc = unsafe {
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, child.pid)
                    .expect("open child process")
            };
            let mut token = HANDLE::default();
            unsafe {
                OpenProcessToken(
                    proc,
                    windows::Win32::Security::TOKEN_QUERY,
                    &mut token,
                )
                .expect("open child token");
            }
            let mut is_appcontainer: u32 = 0;
            let mut len = 0u32;
            unsafe {
                windows::Win32::Security::GetTokenInformation(
                    token,
                    TokenIsAppContainer,
                    Some(&mut is_appcontainer as *mut _ as *mut _),
                    std::mem::size_of::<u32>() as u32,
                    &mut len,
                )
                .expect("query TokenIsAppContainer");
                let _ = CloseHandle(token);
                let _ = CloseHandle(proc);
            }
            assert_eq!(
                is_appcontainer, 1,
                "broker child must run as an AppContainer (TokenIsAppContainer=1)"
            );
        }

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

    /// C6 rounds 6-8 e2e: a PRE-EXISTING ledger file + .lock sidecar (created by
    /// record_launch_pending before spawn, user-only DACL, no package SID) must
    /// be openable+rewritable by the AppContainer child when granted as writable
    /// roots — the consent hook's exact access pattern (open lock, write ledger,
    /// MoveFileExW replace). This is the regression reviewers proved twice
    /// (rounds 6-7); the unit mask test cannot catch it.
    #[test]
    /// C6 round-30: NetPolicy::Loopback grants the per-package loopback
    /// exemption — a sandboxed child can reach a 127.0.0.1 listener. Without
    /// the exemption the AppContainer blocks loopback and this fails.
    #[test]
    fn loopback_policy_allows_localhost_connection() {
        use std::net::TcpListener;
        use std::time::{Duration, Instant};

        let temp = std::env::temp_dir().join(format!(
            "aspis-loopback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();

        // Host-side listener the sandboxed child must be able to reach.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let policy = SandboxPolicy::deny(temp.clone())
            .writable(temp.clone())
            .net(NetPolicy::Loopback);
        // powershell Test-NetConnection returns exit 0 iff the TCP connect
        // succeeds; -InformationLevel Quiet prints True/False.
        let env_vars: Vec<(String, String)> = [
            "PATH", "SystemRoot", "TEMP", "TMP", "USERPROFILE", "COMSPEC",
            "PATHEXT", "APPDATA", "LOCALAPPDATA", "ProgramData", "WINDIR",
        ]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_string(), value)))
        .collect();
        let mut child = spawn_sandboxed(
            &policy,
            "powershell",
            &[
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!(
                    "if (Test-NetConnection -ComputerName 127.0.0.1 -Port {port} -InformationLevel Quiet -WarningAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}"
                ),
            ],
            &temp,
            &env_vars,
        )
        .expect("spawn loopback child");

        // Accept the connection (or time out).
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut accepted = false;
        while Instant::now() < deadline {
            match listener.set_nonblocking(true).and_then(|_| listener.accept()) {
                Ok((_, _)) => {
                    accepted = true;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
        let _ = child.wait_and_restore();
        assert!(
            accepted,
            "sandboxed child with NetPolicy::Loopback must reach a 127.0.0.1 listener"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    fn preexisting_ledger_and_lock_are_writable_in_sandbox() {
        use std::io::{Read, Write};
        use std::os::windows::io::FromRawHandle;

        let temp = std::env::temp_dir().join(format!(
            "devboule_ledger_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).expect("create test dir");
        // Pre-existing ledger + lock, exactly like record_launch_pending leaves them.
        let ledger = temp.join(".aspis-agents.json");
        let lock = temp.join(".aspis-agents.json.lock");
        std::fs::write(&ledger, "{\"v\":1}").expect("write ledger");
        std::fs::write(&lock, "").expect("write lock");

        let policy = SandboxPolicy::deny(temp.clone())
            .writable(temp.clone())
            .writable(ledger.clone())
            .writable(lock.clone())
            .net(crate::backend::sandbox::NetPolicy::Enabled)
            .rlimits(ResourceLimits {
                cpu_secs: 60,
                addr_space_bytes: None,
                max_procs: 16,
            });
        // The hook's real write path is: write a sibling temp, then
        // MoveFileExW(REPLACE_EXISTING) over the ledger (agents.rs
        // replace_file_with_backup -> fs_replace.rs) — which needs DELETE on the
        // target. Exercise the EXACT replace leg with PowerShell Move-Item -Force
        // (atomic replace on Windows). Also plain-truncate the lock (open-for-
        // write). NOTE: no "copy con" — reading the console device inside an
        // AppContainer without a console can hang the child indefinitely
        // (observed: wait hit the 300s RESTORE_WAIT_MS timeout in full-suite
        // runs).
        // `move /y` is cmd's MoveFileExW(REPLACE_EXISTING) — the exact replace
        // leg of fs_replace.rs (NOT PowerShell Move-Item, which cmd /c would
        // silently skip while still exiting 0 on the last `&` command).
        // The source is created BY THE CHILD (echo > tmp), like the hook's
        // sibling temp file — both rename legs run entirely inside the sandbox.
        // DIAGNOSTIC: replace onto a target created BY THE CHILD (not the host
        // pre-existing one) — isolates whether ownership of the target matters.
        // The hook's write path (fs_replace::replace_existing): try atomic
        // MoveFileExW replace; inside the AppContainer that fails ACCESS_DENIED
        // (e2e-proven below), so the copy+delete fallback must work. Exercise
        // the FALLBACK legs exactly: create sibling temp, copy over the
        // pre-existing ledger, delete temp. The ledger content must change.
        let script = format!(
            "echo NEWCONTENT > {} & copy /y {} {} > nul & del /q {} & echo LOCKOK > {}",
            ledger.with_extension("tmp").display(), // sibling temp (hook's temp_path)
            ledger.with_extension("tmp").display(), // source
            ledger.display(),                       // pre-existing target
            ledger.with_extension("tmp").display(), // temp cleanup
            lock.display()
        );
        let mut child = spawn_sandboxed(
            &policy,
            "cmd.exe",
            &["/c".to_string(), script],
            &temp,
            &[],
        )
        .expect("broker spawn");
        // Capture stderr so a failure is diagnosable (cmd reports the real error).
        let stderr_handle = child.take_stderr_handle();
        let mut stderr_file =
            unsafe { std::fs::File::from_raw_handle(stderr_handle.0) };
        let exit = child.wait_and_restore().expect("wait+restore");
        let mut stderr_text = String::new();
        let _ = stderr_file.read_to_string(&mut stderr_text);
        let acl_dump = |p: &std::path::Path| {
            std::process::Command::new("icacls")
                .arg(p)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default()
        };
        eprintln!(
            "[ledger-test] exit={exit} stderr={stderr_text:?}
ledger_acl={}
tmp_acl={}",
            acl_dump(&ledger),
            acl_dump(&ledger.with_extension("tmp"))
        );
        assert_eq!(
            exit, 0,
            "hook-style write must succeed inside the sandbox; stderr: {stderr_text:?}"
        );
        let ledger_text = std::fs::read_to_string(&ledger).unwrap_or_default();
        let lock_text = std::fs::read_to_string(&lock).unwrap_or_default();
        // NEWCONTENT is written to the sibling temp and copied OVER the ledger —
        // present ONLY if the hook's copy+delete fallback (the working replace
        // path inside the AppContainer) succeeded. The atomic MoveFileExW leg is
        // proven broken e2e and documented in fs_replace.rs.
        assert!(
            ledger_text.contains("NEWCONTENT"),
            "ledger must be replaceable via copy+delete fallback, got: {ledger_text:?}"
        );
        assert!(
            lock_text.contains("LOCKOK"),
            "lock sidecar must be writable, got: {lock_text:?}"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// C6 round 7: the writable mask MUST include DELETE + FILE_DELETE_CHILD —
    /// the consent hook's MoveFileExW(REPLACE_EXISTING) ledger replace fails
    /// without them (FILE_GENERIC_WRITE alone lacks both). This is the
    /// regression the reviewer proved; keep it asserted.
    #[test]
    fn writable_mask_includes_delete_rights() {
        let delete = windows::Win32::Storage::FileSystem::DELETE.0;
        let delete_child = FILE_DELETE_CHILD.0;
        assert_ne!(WRITABLE_ACCESS_MASK & delete, 0, "writable must include DELETE");
        assert_ne!(
            WRITABLE_ACCESS_MASK & delete_child, 0,
            "writable must include FILE_DELETE_CHILD"
        );
        // Sanity: it still contains the generic write bits.
        assert_ne!(WRITABLE_ACCESS_MASK & FILE_GENERIC_WRITE.0, 0);
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
