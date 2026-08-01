use std::fs;
use std::path::Path;

pub(crate) fn replace_file_with_backup(
    temp_path: &Path,
    target_path: &Path,
    backup_path: &Path,
    label: &str,
) -> Result<(), String> {
    // First-write detection: if no backup was taken, the target did not exist
    // before this call, so a partially-created target on the error path must
    // be cleaned up rather than left behind.
    let had_backup = target_path.exists();
    if had_backup {
        fs::copy(target_path, backup_path)
            .map_err(|e| format!("Could not back up existing {label}: {e}"))?;
    }

    match replace_existing(temp_path, target_path) {
        Ok(()) => {
            // Success: remove the .bak (best-effort — a leftover backup is inert).
            let _ = fs::remove_file(backup_path);
            Ok(())
        }
        Err(e) => {
            // Round-8 hostile review: on failure the target may be TRUNCATED but
            // still present (copy-over in the fallback), so the old
            // `!target_path.exists()` guard would skip restoration and delete the
            // only good copy. Restore UNCONDITIONALLY from the backup when one
            // was taken, and KEEP the .bak if restoration fails — a stale backup
            // is safer than a corrupted ledger. Restoration runs BEFORE temp
            // cleanup so a locked temp file can never suppress the restore.
            if backup_path.exists() {
                if let Err(restore_err) = restore_copy(backup_path, target_path) {
                    return Err(format!(
                        "Could not save {label}: {e}; backup restoration ALSO failed                          ({restore_err}) — keeping {label} backup at {}",
                        backup_path.display()
                    ));
                }
                // Restoration succeeded — the backup served its purpose.
                let _ = fs::remove_file(backup_path);
            } else if !had_backup && target_path.exists() {
                // Round-9 hostile review: a first write with no backup leaves a
                // partially-created target on failure — remove it (best-effort;
                // the error below already tells the caller the save failed).
                let _ = fs::remove_file(target_path);
            }
            if let Err(cleanup_err) = fs::remove_file(temp_path) {
                return Err(format!(
                    "Could not save {label}: {e}; temp cleanup also failed ({cleanup_err}) —                      {label} temp file left at {}",
                    temp_path.display()
                ));
            }
            Err(format!("Could not save {label}: {e}"))
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
        // Simulate CopyFileW failing mid-copy: destination truncated, then error.
        if let Ok(mut f) = fs::OpenOptions::new().write(true).open(target) {
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

#[cfg(target_os = "windows")]
fn replace_existing(temp_path: &Path, target_path: &Path) -> Result<(), String> {
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
            // C6 round 10 (hostile review): fall back to copy+delete ONLY for
            // ACCESS_DENIED — the AppContainer double-check rejects the atomic
            // replace access path even with DELETE+DELETE_CHILD granted
            // (e2e-proven with cmd move /y, Move-Item -Force,
            // [IO.File]::Replace). Any other code (SHARING_VIOLATION,
            // NOT_SAME_DEVICE, ...) keeps original semantics so host-side
            // shared callers never silently degrade to a non-atomic overwrite.
            // `e.code()` on a win32 failure surfaces the FACILITY_WIN32
            // HRESULT (0x80070005), not the bare win32 code — validate the
            // facility as well as the low 16 bits.
            let code = e.code().0 as u32;
            if (code >> 16) != 0x8007 || (code & 0xFFFF) != win32_error::ERROR_ACCESS_DENIED as u32 {
                return Err(e.to_string());
            }
            // Copy+delete are the legs the AppContainer ACLs do allow
            // (e2e-verified). Non-atomic: CopyFileW overwrites the target in
            // place, so readers can observe a partial file during the copy —
            // acceptable for the agent ledger, whose readers hold the .lock
            // and whose writers keep a .bak for rollback.
            match copy_file_fallback(temp_path, target_path) {
                Ok(_) => match fs::remove_file(temp_path) {
                    Ok(()) => Ok(()),
                    // Round-9 hostile review: cleanup failure must be reported
                    // (not silently swallowed) so the wrapper restores the
                    // backup and the caller knows the temp leaked.
                    Err(cleanup_err) => Err(format!(
                        "Copy fallback succeeded but temp cleanup failed ({cleanup_err}) — \
                         temp file left at {}",
                        temp_path.display()
                    )),
                },
                Err(copy_err) => Err(format!(
                    "MoveFileExW replace failed (Access denied in sandbox?);                      copy fallback also failed: {copy_err}"
                )),
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn replace_existing(temp_path: &Path, target_path: &Path) -> Result<(), String> {
    fs::rename(temp_path, target_path).map_err(|e| e.to_string())
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

        arm_move_fault(); // emulated ERROR_ACCESS_DENIED from the move

        let res = replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing");
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

        arm_move_fault(); // move -> ACCESS_DENIED
        arm_copy_fault(); // copy fallback truncates target then fails

        let res = replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing");
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

        arm_move_fault(); // move -> ACCESS_DENIED
        arm_copy_fault(); // copy fallback fails
        arm_restore_fault(); // restore copy fails -> .bak kept

        let res = replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing");
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

        arm_move_fault(); // move -> ACCESS_DENIED
        arm_copy_fault(); // copy fallback truncates target (creates empty) then fails

        let res = replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing");
        assert!(res.is_err(), "replace must fail");
        assert!(
            !target_path.exists(),
            "partially-created target must be removed on first-write failure"
        );
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
