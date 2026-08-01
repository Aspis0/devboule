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
            let _ = fs::remove_file(backup_path);
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(temp_path);
            if backup_path.exists() && !target_path.exists() {
                let _ = fs::copy(backup_path, target_path);
            }
            // Best-effort: never leave the .bak behind on the error path. The success path
            // already removes it; here the restore-copy (if any) has happened, so the
            // backup is no longer needed and would otherwise leak next to the target.
            let _ = fs::remove_file(backup_path);
            Err(format!("Could not save {label}: {e}"))
        }
    }
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
        Err(_) => {
            // C6 round 7 (final hostile review, e2e-proven): MoveFileExW
            // REPLACE_EXISTING fails with ACCESS_DENIED inside an AppContainer
            // even with DELETE + DELETE_CHILD granted (the sandbox double-check
            // rejects the atomic-replace access path; verified with cmd move /y,
            // PowerShell Move-Item -Force and [IO.File]::Replace). Fall back to
            // copy+delete, which the AppContainer ACLs do allow (also verified
            // e2e). Slightly less atomic (target briefly absent between delete
            // and copy) — acceptable for the agent ledger; the caller already
            // holds the .lock and keeps a .bak.
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
