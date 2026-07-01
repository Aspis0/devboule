//! Persist the orchestrator's cumulative conversation to disk so a RESTARTED session can
//! resume where it left off (v6 Phase 4). std-only, dependency-light, best-effort.

use std::io::Write;
use std::path::Path;

/// Load a previously-persisted conversation from `path`. Returns `None` when the file is
/// missing, unreadable, or empty/blank (i.e. a fresh start — the caller falls back to the
/// launch goal).
pub fn load(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().filter(|s| !s.trim().is_empty())
}

/// Persist `conversation` to `path` ATOMICALLY: write to a sibling temp file
/// (`<path>.tmp`) then rename over `path`, so a crash mid-write never leaves a truncated
/// file. Creates parent dirs if needed. Returns the io::Result for the caller to
/// log-and-ignore (persistence is best-effort, never fatal).
pub fn save(path: &Path, conversation: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut tmp = path.to_path_buf();
    tmp.set_file_name(format!(
        "{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("session")
    ));

    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(conversation.as_bytes())?;
    file.flush()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_load_roundtrip() {
        let id = std::process::id();
        let path = std::env::temp_dir().join(format!("session_persist_test_{id}"));
        let conversation = "hello world\nline 2";

        save(&path, conversation).expect("save should succeed");
        let loaded = load(&path).expect("load should succeed");
        assert_eq!(loaded, conversation);

        // Clean up
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_nonexistent() {
        let path = std::env::temp_dir().join(format!(
            "session_persist_nonexistent_{}",
            std::process::id()
        ));
        assert!(load(&path).is_none());
    }

    #[test]
    fn test_load_blank_file() {
        let id = std::process::id();
        let path = std::env::temp_dir().join(format!("session_persist_blank_{id}"));

        std::fs::write(&path, "   \n  \n").expect("write should succeed");
        assert!(load(&path).is_none());

        // Clean up
        let _ = std::fs::remove_file(&path);
    }
}
