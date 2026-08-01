use std::fs;
use std::path::Path;

pub(crate) fn replace_file_with_backup(
    temp_path: &Path,
    target_path: &Path,
    backup_path: &Path,
    label: &str,
) -> Result<(), String> {
    if target_path.exists() {
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
                if let Err(restore_err) = fs::copy(backup_path, target_path) {
                    return Err(format!(
                        "Could not save {label}: {e}; backup restoration ALSO failed                          ({restore_err}) — keeping {label} backup at {}",
                        backup_path.display()
                    ));
                }
                // Restoration succeeded — the backup served its purpose.
                let _ = fs::remove_file(backup_path);
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
    // MoveFileExW failure codes that justify the copy+delete fallback:
    //  - ERROR_ACCESS_DENIED (5): the AppContainer double-check rejects the
    //    atomic-replace access path even with DELETE+DELETE_CHILD granted
    //    (e2e-proven).
    //  - ERROR_SHARING_VIOLATION (32): transient locks (AV, editors) on the
    //    source/target — copy+delete can still complete the save.
    // Cross-volume (ERROR_NOT_SAME_DEVICE=17) and other errors keep their
    // original semantics: never silently degrade to a non-atomic overwrite.
    pub const ERROR_ACCESS_DENIED: i32 = 5;
    pub const ERROR_SHARING_VIOLATION: i32 = 32;
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

    let replace = unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    match replace {
        Ok(()) => Ok(()),
        Err(e) => {
            // C6 round 9 (hostile review): fall back to copy+delete ONLY for
            // ACCESS_DENIED (AppContainer double-check rejects the atomic
            // replace access path — e2e-proven with cmd move /y, Move-Item
            // -Force, [IO.File]::Replace) and SHARING_VIOLATION (transient
            // locks). Any other error (e.g. cross-volume NOT_SAME_DEVICE)
            // keeps its original semantics instead of silently degrading to a
            // non-atomic overwrite.
            // `e.code()` on a win32 failure surfaces the FACILITY_WIN32
            // HRESULT (0x80070005/0x80070020), not the bare win32 code.
            let code = e.code().0 as u32 & 0xFFFF;
            if code != win32_error::ERROR_ACCESS_DENIED as u32
                && code != win32_error::ERROR_SHARING_VIOLATION as u32
            {
                return Err(e.to_string());
            }
            // Copy+delete are the legs the AppContainer ACLs do allow (e2e-verified).
            // Slightly less atomic (target briefly absent between delete and copy) —
            // acceptable for the agent ledger; the caller already holds the .lock.
            match std::fs::copy(temp_path, target_path) {
                Ok(_) => {
                    let _ = std::fs::remove_file(temp_path);
                    Ok(())
                }
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

    /// Round-9 hostile-review regression: the fallback must fire when the ATOMIC
    /// move is denied (share-locked source: no FILE_SHARE_DELETE on the temp),
    /// and the copy+delete legs must land the new content. Mirrors the
    /// AppContainer case (e2e-proven: MoveFileExW ACCESS_DENIED, copy allowed).
    #[cfg(target_os = "windows")]
    #[test]
    fn atomic_move_denied_falls_back_to_copy() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        fs::write(&target_path, b"OLD").unwrap();
        fs::write(&temp_path, b"NEW").unwrap();

        // Lock the SOURCE with share READ|WRITE but NO DELETE: MoveFileExW
        // fails (sharing violation on the source's delete path), but the copy
        // fallback (needs READ on source, granted) succeeds. The fallback's
        // temp cleanup is best-effort, so the locked source doesn't fail the
        // save.
        let _lock = share_lock(&temp_path, 0x3 /* READ|WRITE, no DELETE */);

        let res = replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing");
        assert!(res.is_ok(), "copy fallback must succeed when move is denied: {res:?}");
        assert_eq!(
            fs::read_to_string(&target_path).unwrap(),
            "NEW",
            "fallback must land the new content"
        );
        // NOTE: the artificial share-lock keeps the temp alive (cleanup is
        // best-effort in the fallback); the real AppContainer case has no open
        // handles, and temp consumption is covered by the success-path test.
        assert!(!backup_path.exists(), "backup removed on success");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-9 regression: if the fallback copy itself fails, the target may be
    /// present but stale/truncated — the backup must be restored UNCONDITIONALLY
    /// (no `!target.exists()` guard) and the .bak consumed only by a successful
    /// restore.
    #[cfg(target_os = "windows")]
    #[test]
    fn failed_replace_restores_backup_even_if_target_still_exists() {
        let dir = tmp_dir();
        let backup_path = dir.join("payload.bak");
        let target_path = dir.join("target.json");
        let temp_path = dir.join("payload.tmp");
        fs::write(&target_path, b"GOOD-OLD").unwrap();
        fs::write(&temp_path, b"NEW").unwrap();

        // Lock the source with NO sharing at all: MoveFileExW fails AND the copy
        // fallback fails, but the target file remains present.
        let _lock = share_lock(&temp_path, 0x0);

        let res = replace_file_with_backup(&temp_path, &target_path, &backup_path, "thing");
        assert!(res.is_err(), "replace must fail: {res:?}");
        let restored = fs::read_to_string(&target_path).unwrap_or_default();
        assert!(
            restored.contains("GOOD-OLD"),
            "backup must be restored unconditionally, got: {restored:?}"
        );
        assert!(
            !backup_path.exists(),
            "backup must be consumed by a successful restore"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-9: if the restore itself fails, the .bak MUST be kept (a stale
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

        // Lock the SOURCE with no sharing (replace + copy fallback fail) AND the
        // TARGET with no WRITE share (restore copy fails). Both locks are held
        // for the whole call, so: backup = OLD (read allowed), replace fails,
        // restore fails -> .bak kept.
        let _src_lock = share_lock(&temp_path, 0x0);
        let _dst_lock = share_lock(&target_path, 0x1 /* READ only */);

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
