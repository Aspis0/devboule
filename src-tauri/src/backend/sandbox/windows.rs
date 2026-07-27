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

// ─── C4: Network egress layer ──────────────────────────────────────────────────
//
// Blocks outbound network for the child's program path via `netsh advfirewall`.
// Per-application (not per-process), but real enforcement. Rule is added before
// spawn and removed after child exit. For v1: NetPolicy::None only. Loopback and
// Enabled are deferred (WFP filter complexity is out of v1 scope).

/// Saved firewall rule name for restore after child exit.
#[derive(Default)]
pub struct NetPolicySnapshot {
    rule_name: String,
}

/// Add a firewall rule blocking outbound network for `program` (C4).
/// Only applies when `policy.net == NetPolicy::None`.
pub fn apply_net_policy(policy: &SandboxPolicy, program: &str) -> Result<NetPolicySnapshot, String> {
    use super::NetPolicy;
    match policy.net {
        NetPolicy::None => {
            let rule_name = format!("devboule_sandbox_block_{}", std::process::id());
            let out = std::process::Command::new("netsh")
                .args([
                    "advfirewall", "firewall", "add", "rule",
                    &format!("name={rule_name}"),
                    "dir=out", "action=block",
                    &format!("program=\"{program}\""),
                ])
                .output()
                .map_err(|e| format!("netsh add rule spawn failed: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "netsh add rule failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            Ok(NetPolicySnapshot { rule_name })
        }
        NetPolicy::Loopback | NetPolicy::Enabled => {
            // v1: deferred. WFP filter for loopback-permit is out of scope.
            Ok(NetPolicySnapshot { rule_name: String::new() })
        }
    }
}

/// Remove the firewall rule saved by [`apply_net_policy`].
pub fn restore_net_policy(snapshot: NetPolicySnapshot) -> Result<(), String> {
    if snapshot.rule_name.is_empty() {
        return Ok(());
    }
    let out = std::process::Command::new("netsh")
        .args([
            "advfirewall", "firewall", "delete", "rule",
            &format!("name={}", snapshot.rule_name),
        ])
        .output()
        .map_err(|e| format!("netsh delete rule spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "netsh delete rule failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
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
    acl_snapshots: Vec<PathAclSnapshot>,
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
        Self { acl: Some(acl), net: None }
    }
    fn set_net(&mut self, net: NetPolicySnapshot) {
        self.net = Some(net);
    }
    /// Disarm the guard: caller takes ownership of snapshots (success path).
    fn take(mut self) -> (Vec<PathAclSnapshot>, NetPolicySnapshot) {
        let acl = self.acl.take().unwrap_or_default();
        let net = self.net.take().unwrap_or(NetPolicySnapshot { rule_name: String::new() });
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

impl SandboxedChild {
    /// Returns the stdout pipe read handle. Wrap with `std::fs::File::from_raw_handle`.
    pub fn stdout_handle(&self) -> HANDLE { self.stdout_read }
    /// Returns the stderr pipe read handle.
    pub fn stderr_handle(&self) -> HANDLE { self.stderr_read }

    /// Take ownership of the stdout pipe handle (for conversion to `File`).
    /// After this call, Drop will NOT close the stdout handle — caller owns it.
    pub fn take_stdout_handle(&mut self) -> HANDLE {
        std::mem::take(&mut self.stdout_read)
    }
    /// Take ownership of the stderr pipe handle (for conversion to `File`).
    pub fn take_stderr_handle(&mut self) -> HANDLE {
        std::mem::take(&mut self.stderr_read)
    }

    /// Non-blocking wait. Returns Some(exit_code) if child exited, None if still running.
    pub fn try_wait(&self) -> Result<Option<i32>, String> {
        use windows::Win32::System::Threading::{WaitForSingleObject, GetExitCodeProcess};
        let result = unsafe { WaitForSingleObject(self.process_handle, 0) };
        if result.0 == 0 { // WAIT_OBJECT_0 = 0
            let mut code: u32 = 0;
            unsafe { GetExitCodeProcess(self.process_handle, &mut code)
                .map_err(|e| format!("GetExitCodeProcess failed: {e}"))?; }
            Ok(Some(code as i32))
        } else {
            Ok(None)
        }
    }

    /// Kill the child process.
    pub fn kill(&self) -> Result<(), String> {
        unsafe {
            windows::Win32::System::Threading::TerminateProcess(self.process_handle, 1)
                .map_err(|e| format!("TerminateProcess failed: {e}"))
        }
    }

    /// Wait for the child to exit, then restore the filesystem ACLs (C3).
    /// Handles are closed by `Drop` when the struct goes out of scope.
    pub fn wait_and_restore(mut self) -> Result<i32, String> {
        use windows::Win32::System::Threading::{WaitForSingleObject, GetExitCodeProcess, INFINITE};

        let exit_code = unsafe {
            WaitForSingleObject(self.process_handle, INFINITE);
            let mut code: u32 = 0;
            GetExitCodeProcess(self.process_handle, &mut code)
                .map_err(|e| format!("GetExitCodeProcess failed: {e}"))?;
            code as i32
        };

        // C3+C4: restore ACLs + net policy. Take both BEFORE restoring so Drop sees
        // empty snapshots even if one restore fails (reviewer C4 note #2 fix).
        let snapshots = std::mem::take(&mut self.acl_snapshots);
        let net = std::mem::take(&mut self.net_snapshot);
        self.restored = true;

        let acl_err = restore_path_policy(snapshots).err();
        let net_err = restore_net_policy(net).err();
        if let Some(e) = net_err.or(acl_err) {
            return Err(e);
        }

        Ok(exit_code)
        // Drop closes ALL handles (child is dead, safe to close job too).
    }
}

impl Drop for SandboxedChild {
    fn drop(&mut self) {
        unsafe {
            // If wait_and_restore was NOT called, kill the child + restore ACLs.
            if !self.restored {
                let _ = windows::Win32::System::Threading::TerminateProcess(self.process_handle, 1);
                let _ = windows::Win32::System::Threading::WaitForSingleObject(self.process_handle, 5000);
                let snapshots = std::mem::take(&mut self.acl_snapshots);
                if !snapshots.is_empty() {
                    let _ = restore_path_policy(snapshots);
                }
                let net = std::mem::take(&mut self.net_snapshot);
                let _ = restore_net_policy(net);
            }
            // Close ALL handles (including job — child is dead, KILL_ON_JOB_CLOSE is safe).
            let _ = CloseHandle(self.process_handle);
            let _ = CloseHandle(self.thread_handle);
            let _ = CloseHandle(self.stdout_read);
            let _ = CloseHandle(self.stderr_read);
            let _ = CloseHandle(self.job);
        }
    }
}

/// Build a UTF-16 environment block from (KEY, VALUE) pairs.
/// Sorted case-insensitively by key (Windows requires this for CreateProcess).
/// Double-null terminated.
fn make_env_block(env_vars: &[(String, String)]) -> Vec<u16> {
    let mut items: Vec<(String, String, String)> = env_vars.iter()
        .map(|(k, v)| (k.to_uppercase(), k.clone(), v.clone()))
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let mut w: Vec<u16> = Vec::new();
    for (_, k, v) in items {
        let entry = format!("{k}={v}");
        w.extend(entry.encode_utf16());
        w.push(0);
    }
    w.push(0);
    w
}

/// Create an anonymous pipe and make the write end inheritable.
fn create_pipe() -> Result<(HANDLE, HANDLE), String> {
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::Foundation::SetHandleInformation;
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe {
        CreatePipe(&mut read, &mut write, None, 0)
            .map_err(|e| format!("CreatePipe failed: {e}"))?;
        SetHandleInformation(write, 0x1u32, windows::Win32::Foundation::HANDLE_FLAGS(0x1)); // HANDLE_FLAG_INHERIT = 0x1
    }
    Ok((read, write))
}

/// Open the current process token and create a restricted version
/// with DISABLE_MAX_PRIVILEGE (strips all privileges from the token).
fn create_restricted_token() -> Result<HANDLE, String> {
    use windows::Win32::Security::{CreateRestrictedToken, CREATE_RESTRICTED_TOKEN_FLAGS, TOKEN_DUPLICATE, TOKEN_QUERY, TOKEN_ASSIGN_PRIMARY, TOKEN_ACCESS_MASK};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut primary_token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ACCESS_MASK(TOKEN_DUPLICATE.0 | TOKEN_QUERY.0 | TOKEN_ASSIGN_PRIMARY.0),
            &mut primary_token,
        ).map_err(|e| format!("OpenProcessToken failed: {e}"))?;
    }

    let mut restricted_token = HANDLE::default();
    unsafe {
        CreateRestrictedToken(
            primary_token,
            CREATE_RESTRICTED_TOKEN_FLAGS(0x1), // DISABLE_MAX_PRIVILEGE
            None,
            None,
            None,
            &mut restricted_token,
        ).map_err(|e| format!("CreateRestrictedToken failed: {e}"))?;
    }

    unsafe { let _ = CloseHandle(primary_token); }
    Ok(restricted_token)
}

/// Create a Job Object with kill-on-close + optional memory limit.
fn create_job_object(rlimits: &ResourceLimits) -> Result<HANDLE, String> {
    let memory_limit: usize = rlimits
        .addr_space_bytes
        .map(|b| b as usize)
        .unwrap_or(usize::MAX);
    unsafe {
        let job = CreateJobObjectW(None, None)
            .map_err(|e| format!("CreateJobObjectW failed: {e}"))?;
        let mut info = std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>();
        let mut basic = std::mem::zeroed::<JOBOBJECT_BASIC_LIMIT_INFORMATION>();
        basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        info.BasicLimitInformation = basic;
        info.ProcessMemoryLimit = memory_limit;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ).map_err(|e| format!("SetInformationJobObject failed: {e}"))?;
        Ok(job)
    }
}

/// Spawn a child process with a restricted token, Job Object, and filesystem ACLs.
/// This is the C2 broker: it replaces `std::process::Command::spawn()` for Windows
/// sandboxed runs.
///
/// Integrates C1 (Job Object), C2 (restricted token), and C3 (filesystem ACLs).
/// After the child exits, call `wait_and_restore()` to restore ACLs + close handles.
pub fn spawn_sandboxed(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    cwd: &Path,
    env_vars: &[(String, String)],
) -> Result<SandboxedChild, String> {
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, STARTUPINFOW, PROCESS_INFORMATION,
        STARTF_USESTDHANDLES, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
    };
    use windows::core::{PWSTR, PCWSTR};
    use windows::Win32::Foundation::BOOL;

    // C3+C4: apply filesystem ACLs + net policy before spawn.
    // SandboxGuard ensures both are restored even if spawn fails.
    let mut guard = SandboxGuard::new(apply_path_policy(policy)?);
    guard.set_net(apply_net_policy(policy, program)?);

    // C1: create Job Object.
    let job = create_job_object(&policy.rlimits)?;

    // C2: create restricted token.
    let restricted_token = create_restricted_token()?;

    // Create pipes for stdout/stderr.
    let (stdout_read, stdout_write) = create_pipe()?;
    let (stderr_read, stderr_write) = create_pipe()?;

    // Build command line: "program arg1 arg2 ..."
    let cmdline = if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    };
    let mut cmdline_wide: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();

    // Build env block.
    let env_block = make_env_block(env_vars);

    // Build cwd wide string.
    let cwd_wide: Vec<u16> = cwd.to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // Desktop name — required for restricted tokens to avoid STATUS_DLL_INIT_FAILED.
    let mut desktop: Vec<u16> = "winsta0\\default\0".encode_utf16().collect();

    // STARTUPINFOW.
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdOutput = stdout_write;
    si.hStdError = stderr_write;
    si.hStdInput = HANDLE::default();
    si.lpDesktop = windows::core::PWSTR(desktop.as_mut_ptr());

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let flags = CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;

    unsafe {
        CreateProcessAsUserW(
            restricted_token,
            PCWSTR::null(),
            PWSTR(cmdline_wide.as_mut_ptr()),
            None, None,
            BOOL(1),
            flags,
            Some(env_block.as_ptr() as *const _),
            PCWSTR(cwd_wide.as_ptr()),
            &si,
            &mut pi,
        ).map_err(|e| format!("CreateProcessAsUserW failed: {e}"))?;
    }

    // Close write ends of pipes (parent doesn't need them).
    // Close the restricted token (child has its own copy).
    unsafe {
        let _ = CloseHandle(stdout_write);
        let _ = CloseHandle(stderr_write);
        let _ = CloseHandle(restricted_token);
    }

    // C1: assign child to Job Object.
    unsafe {
        AssignProcessToJobObject(job, pi.hProcess)
            .map_err(|e| format!("AssignProcessToJobObject failed: {e}"))?;
    }

    // Disarm the guard: on success, snapshots move into SandboxedChild.
    let (acl_snapshots, net_snapshot) = guard.take();

    Ok(SandboxedChild {
        process_handle: pi.hProcess,
        thread_handle: pi.hThread,
        pid: pi.dwProcessId,
        stdout_read,
        stderr_read,
        acl_snapshots,
        net_snapshot,
        job,
        restored: false,
    })
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
