use std::fs;
use std::path::Path;

/// Atomic replace with backup. Host-side shared callers (design/projects/
/// config/oracle saves) MUST NOT pass `allow_copy_fallback` — that capability
/// is reserved for sandboxed ledger writers, where the AppContainer
/// double-check denies MoveFileExW(REPLACE_EXISTING) with ACCESS_DENIED even
/// when DELETE+DELETE_CHILD are granted (e2e-proven). Everywhere else the
/// replace stays strictly atomic or fails.
pub(crate) fn replace_file_with_backup(
    temp_path: &Path,
    target_path: &Path,
    backup_path: &Path,
    label: &str,
) -> Result<(), String> {
    replace_file_with_backup_impl(temp_path, target_path, backup_path, label, false)
}

/// Like `replace_file_with_backup` but with the AppContainer copy+delete
/// fallback capability enabled (see the wrapper doc).
pub(crate) fn replace_file_with_backup_with_fallback(
    temp_path: &Path,
    target_path: &Path,
    backup_path: &Path,
    label: &str,
) -> Result<(), String> {
    replace_file_with_backup_impl(temp_path, target_path, backup_path, label, true)
}

fn replace_file_with_backup_impl(
    temp_path: &Path,
    target_path: &Path,
    backup_path: &Path,
    label: &str,
    allow_copy_fallback: bool,
) -> Result<(), String> {
    // First-write detection: if no backup was taken, the target did not exist
    // before this call, so a partially-created target on the error path must
    // be cleaned up rather than left behind.
    let had_backup = target_path.exists();
    if had_backup {
        fs::copy(target_path, backup_path)
            .map_err(|e| format!("Could not back up existing {label}: {e}"))?;
    }

    match replace_existing(temp_path, target_path, allow_copy_fallback) {
        Ok(()) => {
            // Success: remove the .bak (best-effort — a leftover backup is inert).
            let _ = fs::remove_file(backup_path);
            Ok(())
        }
        Err(f) => {
            if f.target_committed {
                // Round-10 hostile review: the fallback copy LANDED the new
                // content and only the temp cleanup failed. The target holds a
                // valid save — never restore over it or delete it. Report the
                // leak; drop the now-useless .bak.
                let _ = fs::remove_file(backup_path);
                return Err(format!("{label} saved, but temp cleanup failed: {}", f.message));
            }
            // Round-8 hostile review: on failure the target may be TRUNCATED but
            // still present (copy-over in the fallback), so the old
            // `!target_path.exists()` guard would skip restoration and delete the
            // only good copy. Restore UNCONDITIONALLY from the backup when one
            // was taken, and KEEP the .bak if restoration fails — a stale backup
            // is safer than a corrupted ledger. Restoration runs BEFORE temp
            // cleanup so a locked temp file can never suppress the restore.
            if had_backup {
                // Branch on had_backup, NOT on backup_path.exists(): a stale
                // pre-existing backup file at the same path must not be
                // restored over a first-write failure (round-10 review).
                if let Err(restore_err) = restore_copy(backup_path, target_path) {
                    return Err(format!(
                        "Could not save {label}: {}; backup restoration ALSO failed                          ({restore_err}) — keeping {label} backup at {}",
                        f.message,
                        backup_path.display()
                    ));
                }
                // Restoration succeeded — the backup served its purpose.
                let _ = fs::remove_file(backup_path);
            } else if target_path.exists() {
                // Round-9 hostile review: a first write with no backup leaves a
                // partially-created target on failure — remove it (best-effort;
                // the error below already tells the caller the save failed).
                let _ = fs::remove_file(target_path);
            }
            if let Err(cleanup_err) = fs::remove_file(temp_path) {
                return Err(format!(
                    "Could not save {label}: {}; temp cleanup also failed ({cleanup_err}) —                      {label} temp file left at {}",
                    f.message,
                    temp_path.display()
                ));
            }
            Err(format!("Could not save {label}: {}", f.message))
        }
    }
}

#[cfg(target_os = "windows")]
mod win32_error {
    // MoveFileExW failure code that justifies the copy+delete fallback:
    // ERROR_ACCESS_DENIED (5) — the AppContainer double-check rejects the
    // atomic-replace access path even with DELETE+DELETE_CHILD granted
    // (e2e-proven). NO other code falls back: SHARING_VIOLATION (transient
    // AV/editor locks), cross-volume (NOT_SAME_DEVICE), etc. keep their
    // original semantics. This gate matters because replace_file_with_backup
    // is shared with host-side saves (design/projects/config...): only the
    // AppContainer ACCESS_DENIED case may degrade to a non-atomic overwrite,
    // and there the alternative is a FAILED save — the copy+delete legs are
    // the ones the sandbox ACLs demonstrably allow.
    pub const ERROR_ACCESS_DENIED: i32 = 5;
}

/// Copy helper for the fallback, with a test-only fault seam: in tests,
/// `arm_copy_fault()` makes the copy first TRUNCATE the destination (the
/// observed damage mode of a mid-copy I/O failure) and then fail — so the
/// rollback path can be exercised deterministically.
#[cfg(target_os = "windows")]
fn copy_file_fallback(source: &Path, target: &Path) -> Result<u64, String> {
    #[cfg(test)]
    if COPY_FAULT.swap(false, std::sync::atomic::Ordering::SeqCst) {
        // Simulate CopyFileW failing mid-copy: destination created/truncated
        // (create(true) so a first-write partial target is also simulated),
        // then error.
        if let Ok(mut f) = fs::OpenOptions::new().create(true).write(true).open(target) {
            let _ = f.set_len(0);
        }
        return Err("simulated mid-copy failure".to_string());
    }
    fs::copy(source, target).map_err(|e| e.to_string())
}

/// Restore helper with a test-only fault seam (`arm_restore_fault()`), so the
/// "restore fails -> .bak kept" path can be forced deterministically.
#[cfg(target_os = "windows")]
fn restore_copy(source: &Path, target: &Path) -> Result<u64, std::io::Error> {
    #[cfg(test)]
    if RESTORE_FAULT.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "simulated restore failure",
        ));
    }
    fs::copy(source, target)
}

/// MoveFileExW wrapper with a test-only fault seam (`arm_move_fault()`): the
/// seam emulates the AppContainer double-check ACCESS_DENIED (0x80070005)
/// that real MoveFileExW produces in the sandbox (e2e-proven in
/// windows.rs::preexisting_ledger_and_lock test). Unit tests use the seam
/// because the double-check cannot be reproduced with ACLs on a plain host
/// (icacls deny-DELETE is bypassed by DELETE_CHILD on the parent, verified
/// empirically).
#[cfg(target_os = "windows")]
fn move_file_ex(
    source: *const u16,
    target: *const u16,
    flags: windows::Win32::Storage::FileSystem::MOVE_FILE_FLAGS,
) -> windows::core::Result<()> {
    #[cfg(test)]
    if MOVE_FAULT.swap(false, std::sync::atomic::Ordering::SeqCst) {
        // windows-result 0.2: Error::from_win32() takes no args (uses
        // GetLastError); build the HRESULT 0x80070005 explicitly instead.
        return Err(windows::core::Error::from_hresult(
            windows::core::HRESULT(-2147024891i32),
        )); // ERROR_ACCESS_DENIED as HRESULT
    }
    unsafe {
        windows::Win32::Storage::FileSystem::MoveFileExW(
            windows::core::PCWSTR(source),
            windows::core::PCWSTR(target),
            flags,
        )
    }
}

#[cfg(all(test, target_os = "windows"))]
static COPY_FAULT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(all(test, target_os = "windows"))]
static RESTORE_FAULT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(all(test, target_os = "windows"))]
static MOVE_FAULT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(all(test, target_os = "windows"))]
fn arm_copy_fault() {
    COPY_FAULT.store(true, std::sync::atomic::Ordering::SeqCst);
}
#[cfg(all(test, target_os = "windows"))]
fn arm_restore_fault() {
    RESTORE_FAULT.store(true, std::sync::atomic::Ordering::SeqCst);
}
#[cfg(all(test, target_os = "windows"))]
fn arm_move_fault() {
    MOVE_FAULT.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Failure of the replace step. `target_committed` distinguishes the
/// round-10 case: the fallback copy landed the new content and only the temp
/// cleanup failed (target MUST be preserved) from a genuine copy failure
/// (target unreliable, restore required). Platform-neutral because the
/// wrapper and the unix replace_existing share it.
struct ReplaceFailure {
    message: String,
    target_committed: bool,
}

/// True when the CURRENT process runs inside an AppContainer. The fallback
/// capability is enforced by execution context (round-11 hostile review):
/// a bare boolean passed by a host-side caller must not be able to degrade
/// atomic saves — only a process that is ACTUALLY sandboxed (the double-check
/// that motivates the fallback) may use it.
#[cfg(target_os = "windows")]
fn process_is_appcontainer() -> bool {
    #[cfg(test)]
    if APPCONTAINER_SIM.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    unsafe {
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::Security::{GetTokenInformation, TokenIsAppContainer};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        let mut token: HANDLE = HANDLE::default();
        if !OpenProcessToken(GetCurrentProcess(), windows::Win32::Security::TOKEN_QUERY, &mut token)
            .is_ok()
        {
            return false;
        }
        let mut is_app_container = 0u32;
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenIsAppContainer,
            Some(&mut is_app_container as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<u32>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && is_app_container != 0
    }
}

#[cfg(all(test, target_os = "windows"))]
static APPCONTAINER_SIM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(all(test, target_os = "windows"))]
fn arm_appcontainer_sim() {
    APPCONTAINER_SIM.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_os = "windows")]
fn replace_existing(
    temp_path: &Path,
    target_path: &Path,
    allow_copy_fallback: bool,
) -> Result<(), ReplaceFailure> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let mut source: Vec<u16> = temp_path.as_os_str().encode_wide().collect();
    source.push(0);
    let mut target: Vec<u16> = target_path.as_os_str().encode_wide().collect();
    target.push(0);

    let replace = move_file_ex(
        source.as_ptr(),
        target.as_ptr(),
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
    );
    match replace {
        Ok(()) => Ok(()),
        Err(e) => {
            if !allow_copy_fallback || !process_is_appcontainer() {
                // Round-11 hostile review: the copy+delete fallback is a
                // non-atomic capability reserved for ACTUAL sandboxed writers.
                // Two independent gates: the explicit capability flag AND the
                // execution context (this process must itself be inside an
                // AppContainer). A host-side caller that passes the flag by
                // mistake still cannot degrade: 0x80070005 from a plain host
                // (e.g. broken ACLs) is indistinguishable from the
                // AppContainer double-check, so without BOTH gates we keep
                // the original error semantics.
                return Err(ReplaceFailure {
                    message: e.to_string(),
                    target_committed: false,
                });
            }
            // C6 round 10: fall back to copy+delete ONLY for ACCESS_DENIED —
            // the AppContainer double-check rejects the atomic replace access
            // path even with DELETE+DELETE_CHILD granted (e2e-proven with cmd
            // move /y, Move-Item -Force, [IO.File]::Replace). Any other code
            // (SHARING_VIOLATION, NOT_SAME_DEVICE, ...) keeps original
            // semantics. `e.code()` surfaces the FACILITY_WIN32 HRESULT
            // (0x80070005) — validate the facility as well as the low 16 bits.
            let code = e.code().0 as u32;
            if (code >> 16) != 0x8007 || (code & 0xFFFF) != win32_error::ERROR_ACCESS_DENIED as u32 {
                return Err(ReplaceFailure {
                    message: e.to_string(),
                    target_committed: false,
                });
            }
            // Copy+delete are the legs the AppContainer ACLs do allow
            // (e2e-verified). Non-atomic: CopyFileW overwrites the target in
            // place, so readers can observe a partial file during the copy —
            // acceptable for the agent ledger, whose readers hold the .lock
            // and whose writers keep a .bak for rollback.
            match copy_file_fallback(temp_path, target_path) {
                Ok(_) => match fs::remove_file(temp_path) {
                    Ok(()) => Ok(()),
                    // Round-9/10 hostile review: the copy LANDED — this is a
                    // committed save plus a leaked temp, NOT a failure of the
                    // target. target_committed=true keeps the wrapper from
                    // restoring over valid content or deleting it.
                    Err(cleanup_err) => Err(ReplaceFailure {
                        message: format!(
                            "copy fallback committed the target but temp cleanup failed                              ({cleanup_err}); temp file left at {}",
                            temp_path.display()
                        ),
                        target_committed: true,
                    }),
                },
                Err(copy_err) => Err(ReplaceFailure {
                    message: format!(
                        "MoveFileExW replace failed (Access denied in sandbox?);                          copy fallback also failed: {copy_err}"
                    ),
                    target_committed: false,
                }),
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn replace_existing(
    temp_path: &Path,
    target_path: &Path,
    _allow_copy_fallback: bool,
) -> Result<(), ReplaceFailure> {
    // Unix rename is atomic and never hits the AppContainer double-check;
    // the copy+delete fallback capability is a Windows-only concern.
    fs::rename(temp_path, target_path).map_err(|e| ReplaceFailure {
        message: e.to_string(),
        target_committed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;

    fn tmp_dir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "aspis-fsreplace-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_micros()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    /// WARNING 7 regression: on the error branch the backup file must NOT be left behind.
    /// We force `replace_existing` to fail by making the target an EXISTING DIRECTORY
    /// (renaming/moving a file onto a non-empty dir fails on every OS), with a pre-existing
    /// target so a backup is taken first.
    #[test]
    fn error_branch_removes_leaked_backup() {
        let dir = tmp_dir();
        let temp_path = dir.join("payload.tmp");
        let backup_path = dir.join("payload.bak");
        // Target is a directory with content -> rename-over fails deterministically.
        let target_path = dir.join("target");
        fs::create_dir_all(&target_path).unwrap();
        fs::write(target_path.join("keep.txt"), b"x").unwrap();
        fs::write(&temp_path, b"new").unwrap();

        let res = replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing");
        assert!(res.is_err(), "replacing over a non-empty dir must fail");
        assert!(
            !backup_path.exists(),
            "the .bak must be cleaned up on the error path, not leaked"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-10 hostile-review regression: the fallback must fire when the
    /// ATOMIC move is denied with ACCESS_DENIED — emulated via the move-fault
    /// seam because the AppContainer double-check cannot be reproduced with
    /// ACLs on a plain host (icacls deny-DELETE is bypassed by DELETE_CHILD on
    /// the parent; verified empirically). The copy+delete legs must land the
    /// new content.
    #[cfg(target_os = "windows")]
    #[test]
    fn atomic_move_denied_falls_back_to_copy() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        fs::write(&target_path, b"OLD").unwrap();
        fs::write(&temp_path, b"NEW").unwrap();

        arm_appcontainer_sim(); // fallback requires AppContainer context
        arm_move_fault(); // emulated ERROR_ACCESS_DENIED from the move

        let res = replace_file_with_backup_with_fallback(
            &temp_path, &target_path, &backup_path, "thing",
        );
        assert!(res.is_ok(), "copy fallback must succeed when move is denied: {res:?}");
        assert_eq!(
            fs::read_to_string(&target_path).unwrap(),
            "NEW",
            "fallback must land the new content"
        );
        assert!(!temp_path.exists(), "temp must be consumed by the fallback");
        assert!(!backup_path.exists(), "backup removed on success");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-10 regression (DISCRIMINATING for the round-8 bug): the fallback
    /// copy fails MID-COPY (fault seam truncates the target then errors — the
    /// observed damage mode), leaving the target present but truncated. The
    /// backup must be restored UNCONDITIONALLY (the old `!target.exists()`
    /// guard would have skipped restoration and deleted the only good copy,
    /// failing this test) and the .bak consumed only by a successful restore.
    #[cfg(target_os = "windows")]
    #[test]
    fn failed_replace_restores_backup_even_if_target_still_exists() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        fs::write(&target_path, b"GOOD-OLD").unwrap();
        fs::write(&temp_path, b"NEW").unwrap();

        arm_appcontainer_sim(); // fallback requires AppContainer context
        arm_move_fault(); // move -> ACCESS_DENIED
        arm_copy_fault(); // copy fallback truncates target then fails

        let res = replace_file_with_backup_with_fallback(
            &temp_path, &target_path, &backup_path, "thing",
        );
        assert!(res.is_err(), "replace must fail: {res:?}");
        let restored = fs::read_to_string(&target_path).unwrap_or_default();
        assert!(
            restored.contains("GOOD-OLD"),
            "backup must be restored unconditionally over the truncated target, got: {restored:?}"
        );
        assert!(
            !backup_path.exists(),
            "backup must be consumed by a successful restore"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-10: if the restore itself fails, the .bak MUST be kept (a stale
    /// backup is safer than a corrupted ledger) and the error must say so.
    #[cfg(target_os = "windows")]
    #[test]
    fn restore_failure_keeps_backup() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        fs::write(&target_path, b"OLD").unwrap();
        fs::write(&temp_path, b"NEW").unwrap();

        arm_appcontainer_sim(); // fallback requires AppContainer context
        arm_move_fault(); // move -> ACCESS_DENIED
        arm_copy_fault(); // copy fallback fails
        arm_restore_fault(); // restore copy fails -> .bak kept

        let res = replace_file_with_backup_with_fallback(
            &temp_path, &target_path, &backup_path, "thing",
        );
        assert!(res.is_err(), "replace must fail");
        let msg = res.unwrap_err();
        assert!(
            msg.contains("keeping"),
            "error must state the backup is kept, got: {msg}"
        );
        assert!(
            backup_path.exists(),
            ".bak must be KEPT when restoration fails"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-10: the fallback must NOT fire for non-ACCESS_DENIED errors —
    /// host-side shared callers (design/projects/config saves) keep atomic
    /// semantics. A share-locked source yields ERROR_SHARING_VIOLATION, which
    /// must surface as a plain error, NOT a silent copy+delete overwrite.
    #[cfg(target_os = "windows")]
    #[test]
    fn non_access_denied_errors_do_not_fall_back() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        fs::write(&target_path, b"OLD").unwrap();
        fs::write(&temp_path, b"NEW").unwrap();

        // Share-lock the source with NO sharing: move fails with
        // ERROR_SHARING_VIOLATION (not ACCESS_DENIED) -> gate refuses.
        let _lock = share_lock(&temp_path, 0x0);

        let res = replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing");
        assert!(res.is_err(), "non-ACCESS_DENIED must not fall back");
        let msg = res.unwrap_err();
        assert!(
            !msg.contains("copy fallback"),
            "fallback must not have run for sharing violation, got: {msg}"
        );
        // Target untouched (atomic semantics preserved).
        assert_eq!(fs::read_to_string(&target_path).unwrap(), "OLD");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-10: first write with no pre-existing target — on failure the
    /// partially-created target must be removed, not left behind.
    #[cfg(target_os = "windows")]
    #[test]
    fn first_write_failure_cleans_partial_target() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        // NO pre-existing target.
        fs::write(&temp_path, b"NEW").unwrap();

        arm_appcontainer_sim(); // fallback requires AppContainer context
        arm_move_fault(); // move -> ACCESS_DENIED
        arm_copy_fault(); // copy fallback creates+truncates target then fails

        let res = replace_file_with_backup_with_fallback(
            &temp_path, &target_path, &backup_path, "thing",
        );
        assert!(res.is_err(), "replace must fail");
        assert!(
            !target_path.exists(),
            "partially-created target must be removed on first-write failure"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-11 regression: the fallback copy lands the content but the temp
    /// cleanup fails (source share-locked without DELETE after the copy). The
    /// target holds a VALID save — the wrapper must NOT restore over it or
    /// delete it; it reports the leak and drops the .bak.
    #[cfg(target_os = "windows")]
    #[test]
    fn committed_target_with_temp_leak_is_preserved() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        fs::write(&target_path, b"OLD").unwrap();
        fs::write(&temp_path, b"NEW").unwrap();

        arm_appcontainer_sim(); // fallback requires AppContainer context
        arm_move_fault(); // move -> ACCESS_DENIED (emulated double-check)
        // Lock the temp with READ|WRITE but NO DELETE: the fallback copy reads
        // it fine, but the temp cleanup (DELETE) fails -> committed + leak.
        let _lock = share_lock(&temp_path, 0x3);

        let res = replace_file_with_backup_with_fallback(
            &temp_path, &target_path, &backup_path, "thing",
        );
        assert!(res.is_err(), "temp leak must be reported");
        let msg = res.unwrap_err();
        assert!(
            msg.contains("saved, but temp cleanup failed"),
            "must report the leak without claiming save failure, got: {msg}"
        );
        assert_eq!(
            fs::read_to_string(&target_path).unwrap(),
            "NEW",
            "committed target must be preserved, not restored over"
        );
        assert!(!backup_path.exists(), ".bak dropped after a committed save");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-11 regression: WITHOUT the sandbox capability, even a real
    /// ACCESS_DENIED from the move must NOT trigger the copy+delete fallback —
    /// host-side callers keep strictly atomic semantics.
    #[cfg(target_os = "windows")]
    #[test]
    fn access_denied_without_capability_does_not_fall_back() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        fs::write(&target_path, b"OLD").unwrap();
        fs::write(&temp_path, b"NEW").unwrap();

        arm_move_fault(); // ACCESS_DENIED — but NO capability requested

        let res = replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing");
        assert!(res.is_err(), "must fail without the fallback capability");
        let msg = res.unwrap_err();
        assert!(
            !msg.contains("copy fallback"),
            "fallback must not run without the capability, got: {msg}"
        );
        // Old content restored from backup (had_backup=true).
        assert_eq!(fs::read_to_string(&target_path).unwrap(), "OLD");
        assert!(!backup_path.exists(), "backup consumed by successful restore");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-12 hostile review: even WITH the fallback capability requested, a
    /// host-side process (NOT inside an AppContainer) must NOT fall back —
    /// execution context is the load-bearing gate. A host caller that passes
    /// the flag by mistake still gets strictly atomic semantics.
    #[cfg(target_os = "windows")]
    #[test]
    fn capability_without_appcontainer_context_does_not_fall_back() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        fs::write(&target_path, b"OLD").unwrap();
        fs::write(&temp_path, b"NEW").unwrap();

        arm_move_fault(); // ACCESS_DENIED emulated
        // NOTE: no arm_appcontainer_sim() — this process is a plain host.

        let res = replace_file_with_backup_with_fallback(
            &temp_path, &target_path, &backup_path, "thing",
        );
        assert!(res.is_err(), "host context must not fall back");
        let msg = res.unwrap_err();
        assert!(
            !msg.contains("copy fallback"),
            "fallback must not run outside an AppContainer, got: {msg}"
        );
        // Old content restored from backup (had_backup=true).
        assert_eq!(fs::read_to_string(&target_path).unwrap(), "OLD");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Open a handle with a restricted share mode (share = READ|WRITE|DELETE
    /// bitmask). Returns the handle; the caller keeps it alive for the duration
    /// of the failure window.
    #[cfg(target_os = "windows")]
    fn share_lock(path: &Path, share: u32) -> std::os::windows::io::OwnedHandle {
        use std::os::windows::ffi::OsStrExt;
        use std::os::windows::io::FromRawHandle;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE,
        };
        let mut w: Vec<u16> = path.as_os_str().encode_wide().collect();
        w.push(0);
        let h = unsafe {
            CreateFileW(
                PCWSTR(w.as_ptr()),
                0x80000000u32, // GENERIC_READ
                FILE_SHARE_MODE(share),
                None,
                FILE_CREATION_DISPOSITION(3),    // OPEN_EXISTING
                FILE_FLAGS_AND_ATTRIBUTES(0x80), // FILE_ATTRIBUTE_NORMAL
                None,
            )
        }
        .expect("share_lock: CreateFileW must open the pre-created file");
        unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(h.0 as *mut _) }
    }
    /// Guard the UNCHANGED success path: a normal replace removes the backup and writes
    /// the new content.
    #[test]
    fn success_path_replaces_and_removes_backup() {
        let dir = tmp_dir();
        let temp_path = dir.join("payload.tmp");
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.txt");
        fs::write(&target_path, b"old").unwrap();
        fs::write(&temp_path, b"new").unwrap();

        replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing").unwrap();
        assert_eq!(fs::read(&target_path).unwrap(), b"new");
        assert!(!backup_path.exists(), "success path must remove the backup");
        assert!(!temp_path.exists(), "temp must be consumed by the rename");

        let _ = fs::remove_dir_all(&dir);
    }
}
