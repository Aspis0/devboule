//! Project-type and file-language detection for the Censor deterministic engine.
//!
//! Pure logic only (the sole IO is a handful of `Path::exists` manifest probes in
//! `detect_project_kinds`). The A3 orchestrator uses `detect_project_kinds(root)`
//! once per project and `FileLang::from_path(p)` per changed file to pick the
//! applicable runner set (`runners::applicable_runners`).
//!
//! DEAD-CODE NOTE: tested here but first consumed by the A3 orchestrator; the
//! file-scoped allow is removed when A3 wires detection in.
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::Path;

/// What kind(s) of project a root is. A single root can be MULTIPLE kinds (e.g. a
/// Tauri app with a Rust `src-tauri/Cargo.toml` AND a JS `package.json`), so the
/// detector returns a set, not one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectKind {
    Rust,
    Node,
    Python,
}

/// Canonical Python project markers. `pyproject.toml` is the modern standard, but
/// many real projects predate / skip it, so we also accept the classic setuptools
/// markers and the common dependency manifests — otherwise ruff/bandit/vulture
/// never fire on a `requirements.txt`-only or `setup.py`-only repo.
const PYTHON_MARKERS: [&str; 5] = [
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
    "Pipfile",
];

/// Detect which project kinds a root is, from the presence of a canonical manifest
/// at the root: `Cargo.toml` → Rust, `package.json` → Node, and any of
/// [`PYTHON_MARKERS`] → Python. Only the root level is probed (cheap,
/// deterministic); nested manifests in subdirectories are out of scope here — the
/// cross-cutting runners (gitleaks/jscpd/lizard/semgrep) cover files regardless of
/// kind.
pub fn detect_project_kinds(root: &Path) -> HashSet<ProjectKind> {
    let mut kinds = HashSet::new();
    if root.join("Cargo.toml").exists() {
        kinds.insert(ProjectKind::Rust);
    }
    if root.join("package.json").exists() {
        kinds.insert(ProjectKind::Node);
    }
    if PYTHON_MARKERS.iter().any(|m| root.join(m).exists()) {
        kinds.insert(ProjectKind::Python);
    }
    kinds
}

/// Language of a single file, by extension. `Other` covers everything the
/// per-language runners don't apply to (the cross-cutting runners still apply).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileLang {
    Rust,
    Ts,
    Py,
    Other,
}

impl FileLang {
    /// Classify a file path by its extension (case-insensitive). `.ts`/`.tsx`/
    /// `.js`/`.jsx`/`.cts`/`.mts`/`.cjs`/`.mjs` → `Ts` (the JS/TS toolchain:
    /// tsc/eslint/knip); `.rs` → `Rust`; `.py`/`.pyi` → `Py`; else `Other`.
    pub fn from_path(path: &Path) -> FileLang {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_ascii_lowercase(),
            None => return FileLang::Other,
        };
        match ext.as_str() {
            "rs" => FileLang::Rust,
            "ts" | "tsx" | "js" | "jsx" | "cts" | "mts" | "cjs" | "mjs" => FileLang::Ts,
            "py" | "pyi" => FileLang::Py,
            _ => FileLang::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_temp_root(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "aspis-censor-detect-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detect_none_for_empty_dir() {
        let dir = unique_temp_root("empty");
        assert!(detect_project_kinds(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_rust_from_cargo_toml() {
        let dir = unique_temp_root("rust");
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        let kinds = detect_project_kinds(&dir);
        assert!(kinds.contains(&ProjectKind::Rust));
        assert!(!kinds.contains(&ProjectKind::Node));
        assert!(!kinds.contains(&ProjectKind::Python));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_node_from_package_json() {
        let dir = unique_temp_root("node");
        fs::write(dir.join("package.json"), "{}").unwrap();
        let kinds = detect_project_kinds(&dir);
        assert!(kinds.contains(&ProjectKind::Node));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_python_from_pyproject() {
        let dir = unique_temp_root("py");
        fs::write(dir.join("pyproject.toml"), "[project]").unwrap();
        let kinds = detect_project_kinds(&dir);
        assert!(kinds.contains(&ProjectKind::Python));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_python_from_classic_markers() {
        // Each non-pyproject marker independently identifies a Python project.
        for marker in ["setup.py", "setup.cfg", "requirements.txt", "Pipfile"] {
            let dir = unique_temp_root(&format!("py-{}", marker.replace('.', "_")));
            fs::write(dir.join(marker), "x").unwrap();
            let kinds = detect_project_kinds(&dir);
            assert!(
                kinds.contains(&ProjectKind::Python),
                "{marker} should mark a Python project"
            );
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn detect_multiple_kinds_for_polyglot_root() {
        let dir = unique_temp_root("poly");
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        fs::write(dir.join("package.json"), "{}").unwrap();
        fs::write(dir.join("pyproject.toml"), "[project]").unwrap();
        let kinds = detect_project_kinds(&dir);
        assert_eq!(kinds.len(), 3);
        assert!(kinds.contains(&ProjectKind::Rust));
        assert!(kinds.contains(&ProjectKind::Node));
        assert!(kinds.contains(&ProjectKind::Python));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_lang_from_extensions() {
        assert_eq!(FileLang::from_path(Path::new("src/a.rs")), FileLang::Rust);
        assert_eq!(FileLang::from_path(Path::new("src/a.ts")), FileLang::Ts);
        assert_eq!(FileLang::from_path(Path::new("src/a.tsx")), FileLang::Ts);
        assert_eq!(FileLang::from_path(Path::new("src/a.js")), FileLang::Ts);
        assert_eq!(FileLang::from_path(Path::new("src/a.jsx")), FileLang::Ts);
        assert_eq!(FileLang::from_path(Path::new("src/a.mjs")), FileLang::Ts);
        assert_eq!(FileLang::from_path(Path::new("a.py")), FileLang::Py);
        assert_eq!(FileLang::from_path(Path::new("a.pyi")), FileLang::Py);
        assert_eq!(FileLang::from_path(Path::new("a.txt")), FileLang::Other);
        assert_eq!(FileLang::from_path(Path::new("README")), FileLang::Other);
        // Case-insensitive extension matching.
        assert_eq!(FileLang::from_path(Path::new("A.RS")), FileLang::Rust);
        assert_eq!(FileLang::from_path(Path::new("A.PY")), FileLang::Py);
    }
}
