//! Sandboxed tool executor for the agentic loop (Phase 6c, read-only half).
//!
//! Implements `AgentTools` confined to a scope ROOT. SECURITY: the LLM is semi-trusted and
//! its tool arguments are hostile input. Two layers keep every read inside the scope:
//!  1. `safe_rel_path` — pure: rejects absolute paths + `..` traversal, drops `.` components.
//!  2. canonicalize-based checks — a resolved path (and every grep-walked entry) must stay
//!     under the canonical root; SYMLINKS are skipped so a symlinked dir/file inside the
//!     repo can never be followed out of scope (the classic exfiltration vector).
//! Read-only here (read_file / list_dir / grep); edit/write/run are a later chunk that must
//! NOT reuse `resolve` verbatim for writes (see the non-existent-path note on `resolve`).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::backend::agentic_loop::AgentTools;

const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_GREP_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_GREP_DEPTH: usize = 50;
const MAX_GREP_FILES: usize = 2000;
const MAX_GREP_MATCHES: usize = 100;

/// PURE security core: normalize a model-supplied relative path and reject anything that
/// could escape the scope (absolute, drive/scheme `:`, `..`). `.` and empty components are
/// dropped. Returns `"."` for the scope root. NO filesystem access.
pub fn safe_rel_path(rel: &str) -> Result<String, String> {
    let s = rel.trim();
    if s.is_empty() {
        return Err("empty path".to_string());
    }
    if s.starts_with('/') || s.contains(':') {
        return Err("absolute paths not allowed".to_string());
    }
    let mut normalized = s.replace('\\', "/");
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }
    // Re-check for a leading slash AFTER backslash→slash normalization (e.g. "\foo").
    if normalized.starts_with('/') {
        return Err("absolute paths not allowed".to_string());
    }
    let mut parts: Vec<&str> = Vec::new();
    for comp in normalized.split('/') {
        match comp {
            "" | "." => continue,                              // drop empty + current-dir
            ".." => return Err("path escapes scope".to_string()),
            c => parts.push(c),
        }
    }
    if parts.is_empty() {
        return Ok(".".to_string()); // resolved to the scope root
    }
    Ok(parts.join("/"))
}

pub struct ScopedAgentTools {
    root: PathBuf,
}

impl ScopedAgentTools {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn canon_root(&self) -> Result<PathBuf, String> {
        self.root
            .canonicalize()
            .map_err(|_| "scope root is not accessible".to_string())
    }

    /// Resolve a scoped relative path to an absolute path under `root`, verifying (when the
    /// path or its nearest existing ancestor exists) that it canonicalizes UNDER the root.
    /// NOTE: for a fully non-existent path this returns the lexical join WITHOUT a canonical
    /// check — safe for reads (a missing file just fails to open), but the WRITE chunk must
    /// NOT trust this case (refuse, or descend with cap-std).
    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let safe = safe_rel_path(rel)?;
        let full = self.root.join(&safe);
        let canon_root = self.canon_root()?;
        let to_check: Option<PathBuf> = if full.exists() {
            Some(full.clone())
        } else {
            full.parent().filter(|p| p.exists()).map(Path::to_path_buf)
        };
        if let Some(p) = to_check {
            let canon = p.canonicalize().map_err(|e| format!("path could not be resolved: {e}"))?;
            if !canon.starts_with(&canon_root) {
                return Err("path escapes scope".to_string());
            }
        }
        Ok(full)
    }

    fn read_file(&self, path: &str) -> Result<String, String> {
        let p = self.resolve(path)?;
        let content = fs::read_to_string(&p).map_err(|e| e.to_string())?;
        if content.len() <= MAX_READ_BYTES {
            return Ok(content);
        }
        let mut end = MAX_READ_BYTES;
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        Ok(format!("{}\n…[truncated]", &content[..end]))
    }

    fn list_dir(&self, path: &str) -> Result<String, String> {
        let p = self.resolve(path)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&p).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            // file_type() on a DirEntry is lstat — reports the symlink itself, not its target.
            let ft = entry.file_type().map_err(|e| e.to_string())?;
            let suffix = if ft.is_symlink() {
                "@" // mark symlinks; they are not followed
            } else if ft.is_dir() {
                "/"
            } else {
                ""
            };
            entries.push(format!("{}{}", entry.file_name().to_string_lossy(), suffix));
            if entries.len() >= 500 {
                break;
            }
        }
        entries.sort();
        Ok(entries.join("\n"))
    }

    fn grep(&self, pattern: &str, path: &str) -> Result<String, String> {
        let start = self.resolve(path)?;
        let canon_root = self.canon_root()?;
        let mut matches = Vec::new();
        let mut files = 0usize;
        walk_grep(&start, &self.root, &canon_root, pattern, 0, &mut matches, &mut files);
        Ok(matches.join("\n"))
    }
}

const MAX_WRITE_BYTES: usize = 1024 * 1024;

impl ScopedAgentTools {
    /// WRITE-safe resolver (stricter than `resolve`): the parent dir MUST exist and
    /// canonicalize UNDER the root (blocks writing through a symlinked-out ancestor), and
    /// an existing target must NOT be a symlink (blocks overwriting through a symlink-out).
    fn write_resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let safe = safe_rel_path(rel)?;
        if safe == "." {
            return Err("cannot write to the scope root".to_string());
        }
        let full = self.root.join(&safe);
        let parent = full.parent().ok_or_else(|| "invalid path".to_string())?;
        if !parent.exists() {
            return Err("parent directory does not exist".to_string());
        }
        let canon_root = self.canon_root()?;
        let canon_parent = parent.canonicalize().map_err(|e| e.to_string())?;
        if !canon_parent.starts_with(&canon_root) {
            return Err("path escapes scope".to_string());
        }
        if full.exists() {
            if fs::symlink_metadata(&full).map(|m| m.is_symlink()).unwrap_or(true) {
                return Err("refusing to write through a symlink".to_string());
            }
            let canon_full = full.canonicalize().map_err(|e| e.to_string())?;
            if !canon_full.starts_with(&canon_root) {
                return Err("path escapes scope".to_string());
            }
            // A hardlink (nlink > 1) shares an inode with a possibly out-of-scope file;
            // canonicalize resolves dir symlinks, NOT inode aliases, so it can't detect it.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if fs::metadata(&full).map(|m| m.nlink() > 1).unwrap_or(true) {
                    return Err("refusing to write to a hardlinked file".to_string());
                }
            }
        }
        // Residual TOCTOU: a concurrent rename of the verified parent between this check and
        // the actual fs::write could defeat the scope (low risk in a single-user desktop app;
        // not the semi-trusted-LLM threat model). cap-std / openat(O_NOFOLLOW) is the
        // structural fix — a documented follow-up before multi-tenant use.
        Ok(full)
    }

    fn write_file(&self, path: &str, content: &str) -> Result<String, String> {
        if content.len() > MAX_WRITE_BYTES {
            return Err("content too large".to_string());
        }
        let p = self.write_resolve(path)?;
        fs::write(&p, content).map_err(|e| e.to_string())?;
        Ok(format!("wrote {} bytes to {}", content.len(), p.display()))
    }

    fn edit_file(&self, path: &str, old: &str, new: &str) -> Result<String, String> {
        if old.is_empty() {
            return Err("old_string cannot be empty".to_string());
        }
        let p = self.write_resolve(path)?;
        if !p.exists() {
            return Err("file not found".to_string());
        }
        // OOM guard: don't read an arbitrarily large in-scope file into memory.
        if fs::metadata(&p).map(|m| m.len() > MAX_WRITE_BYTES as u64).unwrap_or(true) {
            return Err("file too large to edit".to_string());
        }
        let content = fs::read_to_string(&p).map_err(|e| e.to_string())?;
        let n = content.matches(old).count();
        if n == 0 {
            return Err("old_string not found".to_string());
        }
        if n > 1 {
            return Err(format!("old_string is not unique ({n} matches)"));
        }
        fs::write(&p, content.replacen(old, new, 1)).map_err(|e| e.to_string())?;
        Ok(format!("edited {}", p.display()))
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_grep(
    dir: &Path,
    root: &Path,
    canon_root: &Path,
    pattern: &str,
    depth: usize,
    matches: &mut Vec<String>,
    files: &mut usize,
) {
    if depth > MAX_GREP_DEPTH || *files >= MAX_GREP_FILES || matches.len() >= MAX_GREP_MATCHES {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        if matches.len() >= MAX_GREP_MATCHES || *files >= MAX_GREP_FILES {
            return;
        }
        // SECURITY: never follow symlinks (skip on any file_type error too — fail safe).
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(true) {
            continue;
        }
        let path = entry.path();
        // Defense-in-depth: the real path must stay under the canonical root.
        match path.canonicalize() {
            Ok(c) if c.starts_with(canon_root) => {}
            _ => continue,
        }
        if path.is_dir() {
            walk_grep(&path, root, canon_root, pattern, depth + 1, matches, files);
        } else if path.is_file() {
            // OOM guard: skip files above the size cap rather than loading them.
            if fs::metadata(&path).map(|m| m.len() > MAX_GREP_FILE_BYTES).unwrap_or(true) {
                continue;
            }
            *files += 1;
            if let Ok(content) = fs::read_to_string(&path) {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                for (i, line) in content.lines().enumerate() {
                    if line.contains(pattern) {
                        matches.push(format!("{}:{}: {}", rel, i + 1, line.trim_end()));
                        if matches.len() >= MAX_GREP_MATCHES {
                            return;
                        }
                    }
                }
            }
        }
    }
}

impl AgentTools for ScopedAgentTools {
    fn call(&mut self, name: &str, arguments: &str) -> Result<String, String> {
        let args: Value =
            serde_json::from_str(arguments).map_err(|e| format!("invalid tool arguments JSON: {e}"))?;
        match name {
            "read_file" => {
                let path = args["path"].as_str().ok_or_else(|| "missing 'path'".to_string())?;
                self.read_file(path)
            }
            "list_dir" => self.list_dir(args["path"].as_str().unwrap_or(".")),
            "grep" => {
                let pattern =
                    args["pattern"].as_str().ok_or_else(|| "missing 'pattern'".to_string())?;
                self.grep(pattern, args["path"].as_str().unwrap_or("."))
            }
            "write_file" => {
                let path = args["path"].as_str().ok_or_else(|| "missing 'path'".to_string())?;
                let content =
                    args["content"].as_str().ok_or_else(|| "missing 'content'".to_string())?;
                self.write_file(path, content)
            }
            "edit_file" => {
                let path = args["path"].as_str().ok_or_else(|| "missing 'path'".to_string())?;
                let old =
                    args["oldString"].as_str().ok_or_else(|| "missing 'oldString'".to_string())?;
                let new =
                    args["newString"].as_str().ok_or_else(|| "missing 'newString'".to_string())?;
                self.edit_file(path, old, new)
            }
            other => Err(format!("tool not available: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_rel_path_rejects_escapes() {
        for bad in ["../etc/passwd", "/abs/path", "a/../b", "", "C:\\Windows", "dir/..", "\\foo"] {
            assert!(safe_rel_path(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn safe_rel_path_normalizes_accepted() {
        assert_eq!(safe_rel_path("src/main.rs"), Ok("src/main.rs".to_string()));
        assert_eq!(safe_rel_path("./src/x"), Ok("src/x".to_string()));
        assert_eq!(safe_rel_path("././src/x"), Ok("src/x".to_string()));
        assert_eq!(safe_rel_path("dir//sub"), Ok("dir/sub".to_string()));
        assert_eq!(safe_rel_path("."), Ok(".".to_string()));
        assert_eq!(safe_rel_path("./"), Ok(".".to_string()));
    }

    // FS tests: build a temp scope with an INSIDE file and an OUTSIDE secret reachable only
    // via a symlink, then assert the sandbox never follows the symlink out (the F1 blocker).
    #[cfg(unix)]
    #[test]
    fn grep_does_not_follow_symlinks_out_of_scope() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("aspis_at_{}_{}", std::process::id(), line!()));
        let root = base.join("scope");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("inside.txt"), "INSIDE_MARKER here").unwrap();
        fs::write(outside.join("secret.txt"), "OUTSIDE_SECRET here").unwrap();
        symlink(&outside, root.join("link")).unwrap();

        let mut tools = ScopedAgentTools::new(root.clone());
        let inside = tools.call("grep", r#"{"pattern":"INSIDE_MARKER"}"#).unwrap();
        assert!(inside.contains("INSIDE_MARKER"), "should find in-scope content");
        let leaked = tools.call("grep", r#"{"pattern":"OUTSIDE_SECRET"}"#).unwrap();
        assert!(!leaked.contains("OUTSIDE_SECRET"), "must NOT follow the symlink out of scope");

        // read_file through the symlink must also be blocked.
        let via_link = tools.call("read_file", r#"{"path":"link/secret.txt"}"#);
        assert!(via_link.is_err(), "read via symlink-out must be refused");

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn read_file_truncates_large_content() {
        let base = std::env::temp_dir().join(format!("aspis_at_{}_{}", std::process::id(), line!()));
        fs::create_dir_all(&base).unwrap();
        let big = "x".repeat(MAX_READ_BYTES + 5000);
        fs::write(base.join("big.txt"), &big).unwrap();
        let mut tools = ScopedAgentTools::new(base.clone());
        let out = tools.call("read_file", r#"{"path":"big.txt"}"#).unwrap();
        assert!(out.contains("…[truncated]"));
        assert!(out.len() < big.len());
        let _ = fs::remove_dir_all(&base);
    }
}

#[cfg(all(test, unix))]
mod write_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    // Unique temp dir per call (atomic counter, NOT line!() — tests run in parallel).
    fn unique_root() -> (PathBuf, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("aspis_aw_{}_{}", std::process::id(), n));
        let root = tmp.join("root");
        let outside = tmp.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        (root, tmp)
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (root, tmp) = unique_root();
        let mut tools = ScopedAgentTools::new(root);
        tools.call("write_file", r#"{"path":"test.txt","content":"hello world"}"#).unwrap();
        let back = tools.call("read_file", r#"{"path":"test.txt"}"#).unwrap();
        assert_eq!(back, "hello world");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn edit_replaces_unique_occurrence() {
        let (root, tmp) = unique_root();
        fs::write(root.join("edit.txt"), "original content").unwrap();
        let mut tools = ScopedAgentTools::new(root.clone());
        tools
            .call("edit_file", r#"{"path":"edit.txt","oldString":"original","newString":"updated"}"#)
            .unwrap();
        assert_eq!(fs::read_to_string(root.join("edit.txt")).unwrap(), "updated content");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn edit_rejects_non_unique() {
        let (root, tmp) = unique_root();
        fs::write(root.join("multi.txt"), "a a a").unwrap();
        let mut tools = ScopedAgentTools::new(root);
        let res = tools.call("edit_file", r#"{"path":"multi.txt","oldString":"a","newString":"b"}"#);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("not unique"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_through_symlink_out_is_refused() {
        let (root, tmp) = unique_root();
        std::os::unix::fs::symlink(tmp.join("outside"), root.join("link")).unwrap();
        let mut tools = ScopedAgentTools::new(root);
        let res = tools.call("write_file", r#"{"path":"link/escaped.txt","content":"data"}"#);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("escapes scope"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_to_hardlinked_file_is_refused() {
        let (root, tmp) = unique_root();
        let secret = tmp.join("outside").join("secret.txt");
        fs::write(&secret, "SECRET").unwrap();
        // A hardlink inside the scope sharing the outside file's inode.
        std::fs::hard_link(&secret, root.join("inside_link.txt")).unwrap();
        let mut tools = ScopedAgentTools::new(root);
        let res = tools.call("write_file", r#"{"path":"inside_link.txt","content":"clobber"}"#);
        assert!(res.is_err(), "writing a hardlinked file must be refused");
        assert!(res.unwrap_err().contains("hardlink"));
        // The outside inode must be untouched.
        assert_eq!(fs::read_to_string(&secret).unwrap(), "SECRET");
        let _ = fs::remove_dir_all(&tmp);
    }
}
