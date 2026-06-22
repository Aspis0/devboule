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
    Go,
    Cpp,
    Kotlin,
}

impl ProjectKind {
    /// The canonical list of EVERY [`ProjectKind`] variant, in declaration order. This is
    /// the single source of truth for code that must enumerate all kinds (e.g. the
    /// executor's "what COULD be covered" potential-languages set, which evaluates the gate
    /// against a kinds-set containing every variant).
    ///
    /// EXHAUSTIVENESS: the array is pinned to the enum by [`Self::exhaustive_witness`] (an
    /// exhaustive, wildcard-free match) plus the `all_kinds_*` tests below — adding a new
    /// variant makes the witness match non-exhaustive (FAILS TO COMPILE) and trips the
    /// length/membership tests until the variant is added here. So `ALL` can never silently
    /// drift out of sync with the enum.
    pub const ALL: [ProjectKind; 6] = [
        ProjectKind::Rust,
        ProjectKind::Node,
        ProjectKind::Python,
        ProjectKind::Go,
        ProjectKind::Cpp,
        ProjectKind::Kotlin,
    ];

    /// Identity map over EVERY variant via a wildcard-free `match`. Its only purpose is to
    /// be the compile-time exhaustiveness anchor for [`Self::ALL`]: a newly-added variant
    /// makes this match non-exhaustive and breaks the build, and the tests assert the new
    /// variant is also present in `ALL`. (Never called in non-test code — it carries no
    /// runtime behavior.)
    #[cfg(test)]
    const fn exhaustive_witness(self) -> ProjectKind {
        match self {
            ProjectKind::Rust => ProjectKind::Rust,
            ProjectKind::Node => ProjectKind::Node,
            ProjectKind::Python => ProjectKind::Python,
            ProjectKind::Go => ProjectKind::Go,
            ProjectKind::Cpp => ProjectKind::Cpp,
            ProjectKind::Kotlin => ProjectKind::Kotlin,
        }
    }
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

/// Canonical Kotlin project markers. Kotlin builds with Gradle; the canonical root
/// markers are a Gradle build script (`build.gradle.kts` is the idiomatic Kotlin-DSL
/// form; `build.gradle` covers a Groovy-DSL Gradle project that still has Kotlin
/// sources) or a Gradle settings script (`settings.gradle`/`settings.gradle.kts` —
/// present at a multi-module root even when the build logic lives in submodules). We
/// key off these (not a bare `.kt` heuristic) so a stray `.kt` script in another
/// project doesn't spuriously enable the Kotlin runner — mirrors the
/// canonical-marker choice for Go/C/C++.
const KOTLIN_MARKERS: [&str; 4] = [
    "build.gradle.kts",
    "build.gradle",
    "settings.gradle.kts",
    "settings.gradle",
];

/// Detect which project kinds a root is, from the presence of a canonical manifest
/// at the root: `Cargo.toml` → Rust, `package.json` → Node, any of
/// [`PYTHON_MARKERS`] → Python, `go.mod` → Go, `CMakeLists.txt` OR
/// `compile_commands.json` → C/C++, and any of [`KOTLIN_MARKERS`] (a Gradle
/// build/settings script) → Kotlin. Only the root level is probed (cheap,
/// deterministic); nested manifests in subdirectories are out of scope here — the
/// cross-cutting runners (gitleaks/jscpd/lizard/semgrep) cover files regardless of
/// kind.
///
/// NOTE: HTML has NO project kind — there is no canonical HTML project manifest, so
/// the HTML runner (tidy) gates on [`FileLang::Html`] ALONE in
/// `runners::applicable_runners` (an `.html` file anywhere is checkable), not on a
/// fabricated `ProjectKind`.
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
    // `go.mod` is the canonical module manifest for a Go project (modules are the
    // standard since Go 1.11). A GOPATH-only layout with no `go.mod` is legacy and
    // rare; we deliberately key off the modern marker only (mirrors the
    // single-canonical-marker choice for Rust/Node).
    if root.join("go.mod").exists() {
        kinds.insert(ProjectKind::Go);
    }
    // A C/C++ project has no single universal manifest, but the two canonical, tool-
    // agnostic markers are `CMakeLists.txt` (the de-facto cross-platform build
    // generator) and `compile_commands.json` (the clang compilation database many
    // tools — including cppcheck — consume). Either at the root flags a C/C++ project;
    // we deliberately key off these (not a bare `.c`/`.cpp` heuristic) so a stray C
    // source in another project doesn't spuriously enable the C/C++ runners.
    if root.join("CMakeLists.txt").exists() || root.join("compile_commands.json").exists() {
        kinds.insert(ProjectKind::Cpp);
    }
    // A Kotlin project is identified by a Gradle build/settings script at the root
    // (see [`KOTLIN_MARKERS`]); we deliberately key off the canonical Gradle markers
    // (not a bare `.kt`/`.kts` heuristic) so a stray Kotlin script doesn't spuriously
    // enable ktlint.
    if KOTLIN_MARKERS.iter().any(|m| root.join(m).exists()) {
        kinds.insert(ProjectKind::Kotlin);
    }
    kinds
}

/// Canonical persona-language keys, HIGHEST priority first. Picks ONE primary language for
/// the (role × language) prompt persona from the (possibly multi-kind) project detection or
/// a task's file set.
const LANG_PRIORITY: &[(ProjectKind, &str)] = &[
    (ProjectKind::Rust, "rust"),
    (ProjectKind::Node, "node"),
    (ProjectKind::Python, "python"),
    (ProjectKind::Go, "go"),
    (ProjectKind::Cpp, "cpp"),
    (ProjectKind::Kotlin, "kotlin"),
];

/// Map a per-file language to a canonical persona key, or None for languages with no persona
/// layer (HTML/Shell/YAML/SQL/CSS/Dockerfile/GithubActions/Other). `Ts` → "node" (the JS/TS
/// ecosystem shares the Node persona).
fn file_lang_to_key(l: FileLang) -> Option<&'static str> {
    match l {
        FileLang::Rust => Some("rust"),
        FileLang::Ts => Some("node"),
        FileLang::Py => Some("python"),
        FileLang::Go => Some("go"),
        FileLang::Cpp => Some("cpp"),
        FileLang::Kotlin => Some("kotlin"),
        // No per-language persona for these (covered by the role/base prompt, not a coder
        // persona). EXHAUSTIVE on purpose: a future FileLang variant must be classified here
        // (compile error) instead of being silently swallowed by a `_` wildcard.
        FileLang::Html
        | FileLang::Shell
        | FileLang::Yaml
        | FileLang::Sql
        | FileLang::Dockerfile
        | FileLang::GithubActions
        | FileLang::Css
        | FileLang::Other => None,
    }
}

/// The project's PRIMARY persona language: the highest-priority detected kind, or None when
/// no persona-backed kind is present (fail-open ⇒ no language layer).
pub fn primary_language_from_kinds(kinds: &HashSet<ProjectKind>) -> Option<&'static str> {
    LANG_PRIORITY
        .iter()
        .find_map(|(kind, key)| kinds.contains(kind).then_some(*key))
}

/// The persona language of a TASK's file set: the most frequent per-file language, ties
/// broken by `LANG_PRIORITY`. None when no file maps to a persona key.
pub fn primary_language_from_files(files: &[String]) -> Option<&'static str> {
    let mut counts: std::collections::HashMap<&'static str, usize> = std::collections::HashMap::new();
    for f in files {
        if let Some(key) = file_lang_to_key(FileLang::from_path(Path::new(f))) {
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    let max_count = counts.values().max().copied()?;
    LANG_PRIORITY
        .iter()
        .find_map(|(_, key)| (counts.get(key).copied() == Some(max_count)).then_some(*key))
}

#[cfg(test)]
mod lang_key_tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn primary_language_from_kinds_uses_priority() {
        let mut kinds = HashSet::new();
        kinds.insert(ProjectKind::Rust);
        kinds.insert(ProjectKind::Node);
        assert_eq!(primary_language_from_kinds(&kinds), Some("rust"));
        let mut py = HashSet::new();
        py.insert(ProjectKind::Python);
        assert_eq!(primary_language_from_kinds(&py), Some("python"));
        assert_eq!(primary_language_from_kinds(&HashSet::new()), None);
    }

    #[test]
    fn primary_language_from_files_mode_and_tiebreak() {
        assert_eq!(
            primary_language_from_files(&["a.py".into(), "b.py".into(), "c.rs".into()]),
            Some("python")
        );
        assert_eq!(primary_language_from_files(&["x.ts".into()]), Some("node"));
        assert_eq!(
            primary_language_from_files(&["r.md".into(), "n.txt".into()]),
            None
        );
        assert_eq!(
            primary_language_from_files(&["a.rs".into(), "b.py".into()]),
            Some("rust")
        );
    }

    #[test]
    fn primary_language_from_files_empty_and_balanced_tie() {
        assert_eq!(primary_language_from_files(&[]), None);
        // 2-vs-2 tie → broken by priority (rust > node).
        assert_eq!(
            primary_language_from_files(&[
                "a.rs".into(),
                "b.rs".into(),
                "a.ts".into(),
                "b.ts".into()
            ]),
            Some("rust")
        );
    }

    #[test]
    fn gradle_kts_build_script_is_not_counted_as_kotlin() {
        // `*.gradle.kts` classifies as Other in FileLang::from_path, so it must NOT cast a
        // "kotlin" vote — the two real .kt files decide it.
        assert_eq!(
            primary_language_from_files(&[
                "build.gradle.kts".into(),
                "Main.kt".into(),
                "Lib.kt".into()
            ]),
            Some("kotlin")
        );
    }

    #[test]
    fn lang_priority_covers_every_project_kind() {
        // Future-proofing: a new ProjectKind added to ALL must also get a LANG_PRIORITY entry,
        // else primary_language_from_kinds would silently never return it.
        assert_eq!(LANG_PRIORITY.len(), ProjectKind::ALL.len());
        for k in ProjectKind::ALL {
            assert!(
                LANG_PRIORITY.iter().any(|(pk, _)| *pk == k),
                "ProjectKind::{k:?} missing from LANG_PRIORITY"
            );
        }
    }
}

/// Language of a single file, by extension. `Other` covers everything the
/// per-language runners don't apply to (the cross-cutting runners still apply).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileLang {
    Rust,
    Ts,
    Py,
    Go,
    Cpp,
    Html,
    Kotlin,
    // Lint-runner-only quick wins (no tree-sitter grammar wired — see extract.rs):
    // shell scripts (shellcheck), YAML (yamllint), SQL (sqlfluff). These have no
    // canonical project manifest, so their runners gate on the FileLang ALONE
    // (like HTML), and `parse_file`/`extract_items` return EMPTY items + an empty
    // identifier set for them, so symbol grounding stays disabled while the
    // universal line-range grounding still applies.
    Shell,
    Yaml,
    Sql,
    // Lint-runner-only quick wins with NON-EXTENSION detection (no tree-sitter grammar):
    // Dockerfile (hadolint) is detected by FILE NAME; GithubActions (actionlint) is
    // detected by PATH (a YAML file under `.github/workflows/`); CSS (stylelint) is the
    // `.css`/`.scss`/`.sass`/`.less` family. Like Shell/Yaml/Sql they gate their runner on
    // the FileLang ALONE and `parse_file`/`extract_items` return EMPTY for them.
    Dockerfile,
    GithubActions,
    Css,
    Other,
}

impl FileLang {
    /// Classify a file path by its extension (case-insensitive). `.ts`/`.tsx`/
    /// `.js`/`.jsx`/`.cts`/`.mts`/`.cjs`/`.mjs` → `Ts` (the JS/TS toolchain:
    /// tsc/eslint/knip); `.rs` → `Rust`; `.py`/`.pyi` → `Py`; `.go` → `Go`
    /// (the gofmt/go-vet toolchain); the C/C++ family
    /// `.cpp`/`.cc`/`.cxx`/`.c++`/`.hpp`/`.hh`/`.hxx`/`.h++`/`.c`/`.h` → `Cpp` (the
    /// cppcheck toolchain — C and C++ share one [`FileLang`] since the wired grammar
    /// and the runner both span the family); `.html`/`.htm` → `Html` (the HTML Tidy
    /// toolchain); `.kt`/`.kts` → `Kotlin` (the ktlint toolchain);
    /// `.sh`/`.bash`/`.ksh`/`.zsh` → `Shell` (the shellcheck toolchain);
    /// `.sql` → `Sql` (the sqlfluff toolchain); the CSS family
    /// `.css`/`.scss`/`.sass`/`.less` → `Css` (the stylelint toolchain).
    ///
    /// NON-EXTENSION detection (checked BEFORE the extension match, since these
    /// classifications take precedence over the bare extension):
    ///   - `Dockerfile` is detected by FILE NAME (Dockerfiles have no canonical
    ///     extension): the name is exactly `Dockerfile`/`Containerfile`, OR ends
    ///     with `.dockerfile` (e.g. `foo.dockerfile`), OR is `Dockerfile.<suffix>`
    ///     (e.g. `Dockerfile.prod`) / `<prefix>.Dockerfile` → `Dockerfile`
    ///     (the hadolint toolchain).
    ///   - `GithubActions` is detected by PATH: a `.yml`/`.yaml` file whose path runs
    ///     through a `.github/workflows/` directory → `GithubActions` (the actionlint
    ///     toolchain). A `.yml`/`.yaml` file NOT under `.github/workflows/` stays
    ///     `Yaml` (the yamllint toolchain). GithubActions takes precedence so a
    ///     workflow file is never double-classified.
    ///
    /// Anything else falls through to the extension match: `.yml`/`.yaml` → `Yaml`;
    /// else `Other`.
    pub fn from_path(path: &Path) -> FileLang {
        // FILENAME-based: Dockerfile / Containerfile (no canonical extension). Checked
        // first so a `Dockerfile.prod` (which has the `prod` "extension") is classified
        // by name, not by the bogus extension.
        if is_dockerfile_name(path) {
            return FileLang::Dockerfile;
        }
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_ascii_lowercase(),
            None => return FileLang::Other,
        };
        // PATH-based: a YAML file under `.github/workflows/` is a GitHub Actions
        // workflow (actionlint), not plain YAML (yamllint). Checked before the
        // extension match so GithubActions wins over Yaml for workflow files.
        if matches!(ext.as_str(), "yml" | "yaml") && is_under_github_workflows(path) {
            return FileLang::GithubActions;
        }
        // FILENAME-based: a Gradle Kotlin DSL script (`*.gradle.kts` — `build.gradle.kts`,
        // `settings.gradle.kts`, convention-plugin scripts) is a BUILD script, not Kotlin
        // application source. ktlint on a Gradle DSL script is FP noise (it flags the DSL's
        // builder syntax), so we classify it as `Other` (no per-file runner). Checked before
        // the extension match so it wins over the bare `.kts` → Kotlin mapping. Application
        // `.kt` and a non-Gradle `.kts` script stay Kotlin.
        if matches!(ext.as_str(), "kts") && is_gradle_kts_name(path) {
            return FileLang::Other;
        }
        match ext.as_str() {
            "rs" => FileLang::Rust,
            "ts" | "tsx" | "js" | "jsx" | "cts" | "mts" | "cjs" | "mjs" => FileLang::Ts,
            "py" | "pyi" => FileLang::Py,
            "go" => FileLang::Go,
            "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "c" | "h" => {
                FileLang::Cpp
            }
            "html" | "htm" => FileLang::Html,
            "kt" | "kts" => FileLang::Kotlin,
            // Shell-script family (shellcheck). Extensions are enough; we deliberately
            // skip a `.bashrc`/`.zshrc` filename heuristic (shellcheck on a dotfile is a
            // marginal case and the filename check adds complexity for little gain).
            "sh" | "bash" | "ksh" | "zsh" => FileLang::Shell,
            "yml" | "yaml" => FileLang::Yaml,
            "sql" => FileLang::Sql,
            // CSS family (stylelint): plain CSS plus the SCSS/Sass/Less preprocessors,
            // all of which stylelint lints with the appropriate syntax.
            "css" | "scss" | "sass" | "less" => FileLang::Css,
            _ => FileLang::Other,
        }
    }
}

/// Is `path`'s FILE NAME a Dockerfile, by the conventional naming patterns? Matches
/// (case-insensitively): an exact `Dockerfile`/`Containerfile`; a name ending in
/// `.dockerfile` (e.g. `foo.dockerfile`); a `Dockerfile.<suffix>` (e.g.
/// `Dockerfile.prod`); or a `<prefix>.Dockerfile`. PURE — inspects only the final path
/// component, never the filesystem.
fn is_dockerfile_name(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    let lower = name.to_ascii_lowercase();
    lower == "dockerfile"
        || lower == "containerfile"
        || lower.ends_with(".dockerfile")
        || lower.ends_with(".containerfile")
        || lower.starts_with("dockerfile.")
        || lower.starts_with("containerfile.")
}

/// Does `path` run through a `.github/workflows/` directory (the canonical location of
/// GitHub Actions workflow files)? Scans the path COMPONENTS for an adjacent
/// `.github` → `workflows` pair (case-insensitive on the segment names, matching the
/// rest of this module's casing tolerance). PURE — never touches the filesystem.
fn is_under_github_workflows(path: &Path) -> bool {
    use std::path::Component;
    // Collect the normal (named) path segments, lowercased, in order — INCLUDING the
    // filename. The windows(2) scan for a consecutive `.github` → `workflows` pair is safe
    // with the filename present: a workflow file is never itself the bare segment
    // `workflows`, so the filename can't create a false match. Any such pair qualifies.
    let segs: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str().map(|s| s.to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    segs.windows(2)
        .any(|w| w[0] == ".github" && w[1] == "workflows")
}

/// Is `path`'s FILE NAME a Gradle Kotlin DSL script (`*.gradle.kts`)? Matches
/// (case-insensitively) any name ending in `.gradle.kts` — `build.gradle.kts`,
/// `settings.gradle.kts`, and convention-plugin scripts like `my-plugin.gradle.kts`.
/// These are Gradle BUILD scripts (builder DSL), not Kotlin application source, so the
/// caller classifies them as `Other` to keep ktlint's DSL-syntax FP noise off them. A
/// plain `.kts` (a standalone Kotlin script) is NOT matched and stays Kotlin. PURE —
/// inspects only the final path component, never the filesystem.
fn is_gradle_kts_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_lowercase().ends_with(".gradle.kts"))
        .unwrap_or(false)
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
    fn all_project_kinds_is_exhaustive_and_unique() {
        use std::collections::HashSet;
        // Every entry round-trips through the wildcard-free witness match (identity), and
        // the set is duplicate-free. The REAL guard is the combination: a variant added to
        // the enum makes `exhaustive_witness`'s match non-exhaustive (compile error) and,
        // once handled there, this membership assertion forces it into `ProjectKind::ALL`.
        let set: HashSet<ProjectKind> = ProjectKind::ALL.into_iter().collect();
        assert_eq!(set.len(), ProjectKind::ALL.len(), "ALL has duplicates");
        for k in ProjectKind::ALL {
            assert_eq!(k.exhaustive_witness(), k);
            assert!(set.contains(&k.exhaustive_witness()));
        }
    }

    #[test]
    fn each_known_variant_is_present_in_all() {
        // Spell out each known variant explicitly so that adding a variant to the enum
        // (and to `exhaustive_witness`) but FORGETTING it in `ProjectKind::ALL` is caught:
        // the variant would not be in `ALL`, so this assertion (driven by the witness over
        // `ALL`) plus the exhaustive witness match together fail.
        for k in [
            ProjectKind::Rust,
            ProjectKind::Node,
            ProjectKind::Python,
            ProjectKind::Go,
            ProjectKind::Cpp,
            ProjectKind::Kotlin,
        ] {
            assert!(
                ProjectKind::ALL.contains(&k),
                "ProjectKind::{k:?} missing from ProjectKind::ALL"
            );
        }
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
    fn detect_go_from_go_mod() {
        let dir = unique_temp_root("go");
        fs::write(dir.join("go.mod"), "module example.com/x\n\ngo 1.22\n").unwrap();
        let kinds = detect_project_kinds(&dir);
        assert!(kinds.contains(&ProjectKind::Go));
        assert!(!kinds.contains(&ProjectKind::Rust));
        assert!(!kinds.contains(&ProjectKind::Node));
        assert!(!kinds.contains(&ProjectKind::Python));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_cpp_from_cmakelists_or_compile_commands() {
        // Either canonical marker independently identifies a C/C++ project.
        for marker in ["CMakeLists.txt", "compile_commands.json"] {
            let dir = unique_temp_root(&format!("cpp-{}", marker.replace('.', "_")));
            fs::write(dir.join(marker), "x").unwrap();
            let kinds = detect_project_kinds(&dir);
            assert!(
                kinds.contains(&ProjectKind::Cpp),
                "{marker} should mark a C/C++ project"
            );
            assert!(!kinds.contains(&ProjectKind::Rust));
            assert!(!kinds.contains(&ProjectKind::Go));
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn detect_kotlin_from_gradle_markers() {
        // Each canonical Gradle marker independently identifies a Kotlin project.
        for marker in [
            "build.gradle.kts",
            "build.gradle",
            "settings.gradle.kts",
            "settings.gradle",
        ] {
            let dir = unique_temp_root(&format!("kotlin-{}", marker.replace('.', "_")));
            fs::write(dir.join(marker), "x").unwrap();
            let kinds = detect_project_kinds(&dir);
            assert!(
                kinds.contains(&ProjectKind::Kotlin),
                "{marker} should mark a Kotlin project"
            );
            assert!(!kinds.contains(&ProjectKind::Rust));
            assert!(!kinds.contains(&ProjectKind::Go));
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn detect_multiple_kinds_for_polyglot_root() {
        let dir = unique_temp_root("poly");
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        fs::write(dir.join("package.json"), "{}").unwrap();
        fs::write(dir.join("pyproject.toml"), "[project]").unwrap();
        fs::write(dir.join("go.mod"), "module example.com/x\n").unwrap();
        fs::write(dir.join("CMakeLists.txt"), "project(x)\n").unwrap();
        fs::write(dir.join("build.gradle.kts"), "plugins {}\n").unwrap();
        let kinds = detect_project_kinds(&dir);
        assert_eq!(kinds.len(), 6);
        assert!(kinds.contains(&ProjectKind::Rust));
        assert!(kinds.contains(&ProjectKind::Node));
        assert!(kinds.contains(&ProjectKind::Python));
        assert!(kinds.contains(&ProjectKind::Go));
        assert!(kinds.contains(&ProjectKind::Cpp));
        assert!(kinds.contains(&ProjectKind::Kotlin));
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
        assert_eq!(FileLang::from_path(Path::new("main.go")), FileLang::Go);
        // C/C++ family — C++ sources/headers AND C sources/headers all map to Cpp.
        assert_eq!(FileLang::from_path(Path::new("a.cpp")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("a.cc")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("a.cxx")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("a.c++")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("a.hpp")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("a.hh")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("a.hxx")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("a.h++")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("a.c")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("a.h")), FileLang::Cpp);
        // HTML.
        assert_eq!(FileLang::from_path(Path::new("index.html")), FileLang::Html);
        assert_eq!(FileLang::from_path(Path::new("page.htm")), FileLang::Html);
        // Kotlin — application sources (`.kt`) and a standalone Kotlin script (`.kts`).
        assert_eq!(FileLang::from_path(Path::new("Main.kt")), FileLang::Kotlin);
        assert_eq!(FileLang::from_path(Path::new("script.kts")), FileLang::Kotlin);
        // A Gradle Kotlin DSL build script (`*.gradle.kts`) is NOT Kotlin source — it is a
        // builder-DSL build file, so it maps to `Other` (no ktlint FP noise). Covers the
        // canonical build/settings scripts AND convention-plugin scripts.
        assert_eq!(
            FileLang::from_path(Path::new("build.gradle.kts")),
            FileLang::Other
        );
        assert_eq!(
            FileLang::from_path(Path::new("settings.gradle.kts")),
            FileLang::Other
        );
        assert_eq!(
            FileLang::from_path(Path::new("gradle/my-plugin.gradle.kts")),
            FileLang::Other
        );
        // Case-insensitive: a SHOUTED Gradle DSL name is still `Other`.
        assert_eq!(
            FileLang::from_path(Path::new("BUILD.GRADLE.KTS")),
            FileLang::Other
        );
        // Shell scripts (shellcheck) — the whole `.sh`/`.bash`/`.ksh`/`.zsh` family.
        assert_eq!(FileLang::from_path(Path::new("deploy.sh")), FileLang::Shell);
        assert_eq!(FileLang::from_path(Path::new("lib.bash")), FileLang::Shell);
        assert_eq!(FileLang::from_path(Path::new("old.ksh")), FileLang::Shell);
        assert_eq!(FileLang::from_path(Path::new("env.zsh")), FileLang::Shell);
        // YAML (yamllint).
        assert_eq!(FileLang::from_path(Path::new("ci.yml")), FileLang::Yaml);
        assert_eq!(FileLang::from_path(Path::new("conf.yaml")), FileLang::Yaml);
        // SQL (sqlfluff).
        assert_eq!(FileLang::from_path(Path::new("schema.sql")), FileLang::Sql);
        // CSS family (stylelint) — plain CSS plus the preprocessors.
        assert_eq!(FileLang::from_path(Path::new("a.css")), FileLang::Css);
        assert_eq!(FileLang::from_path(Path::new("a.scss")), FileLang::Css);
        assert_eq!(FileLang::from_path(Path::new("a.sass")), FileLang::Css);
        assert_eq!(FileLang::from_path(Path::new("a.less")), FileLang::Css);
        assert_eq!(FileLang::from_path(Path::new("a.txt")), FileLang::Other);
        assert_eq!(FileLang::from_path(Path::new("README")), FileLang::Other);
        // Case-insensitive extension matching.
        assert_eq!(FileLang::from_path(Path::new("A.RS")), FileLang::Rust);
        assert_eq!(FileLang::from_path(Path::new("A.PY")), FileLang::Py);
        assert_eq!(FileLang::from_path(Path::new("MAIN.GO")), FileLang::Go);
        assert_eq!(FileLang::from_path(Path::new("MAIN.CPP")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("HDR.H")), FileLang::Cpp);
        assert_eq!(FileLang::from_path(Path::new("INDEX.HTML")), FileLang::Html);
        assert_eq!(FileLang::from_path(Path::new("MAIN.KT")), FileLang::Kotlin);
        assert_eq!(FileLang::from_path(Path::new("DEPLOY.SH")), FileLang::Shell);
        assert_eq!(FileLang::from_path(Path::new("CI.YML")), FileLang::Yaml);
        assert_eq!(FileLang::from_path(Path::new("SCHEMA.SQL")), FileLang::Sql);
        assert_eq!(FileLang::from_path(Path::new("A.CSS")), FileLang::Css);
        assert_eq!(FileLang::from_path(Path::new("A.SCSS")), FileLang::Css);
    }

    #[test]
    fn dockerfile_is_detected_by_filename_not_extension() {
        // Exact canonical names (no extension).
        assert_eq!(
            FileLang::from_path(Path::new("Dockerfile")),
            FileLang::Dockerfile
        );
        assert_eq!(
            FileLang::from_path(Path::new("Containerfile")),
            FileLang::Dockerfile
        );
        // Nested in a path.
        assert_eq!(
            FileLang::from_path(Path::new("docker/Dockerfile")),
            FileLang::Dockerfile
        );
        // `Dockerfile.<suffix>` (e.g. an environment-specific build).
        assert_eq!(
            FileLang::from_path(Path::new("Dockerfile.prod")),
            FileLang::Dockerfile
        );
        // `<prefix>.dockerfile` extension form.
        assert_eq!(
            FileLang::from_path(Path::new("foo.dockerfile")),
            FileLang::Dockerfile
        );
        assert_eq!(
            FileLang::from_path(Path::new("app.Dockerfile")),
            FileLang::Dockerfile
        );
        // Containerfile variants (Podman) — symmetric with the Dockerfile forms.
        assert_eq!(
            FileLang::from_path(Path::new("Containerfile.prod")),
            FileLang::Dockerfile
        );
        assert_eq!(
            FileLang::from_path(Path::new("base.Containerfile")),
            FileLang::Dockerfile
        );
        // Case-insensitive on the whole name.
        assert_eq!(
            FileLang::from_path(Path::new("DOCKERFILE")),
            FileLang::Dockerfile
        );
        // A file that merely CONTAINS "dockerfile" in the middle is NOT a Dockerfile.
        assert_eq!(
            FileLang::from_path(Path::new("my-dockerfile-notes.txt")),
            FileLang::Other
        );
    }

    #[test]
    fn github_workflow_yaml_is_actions_but_plain_yaml_is_yaml() {
        // A YAML file under `.github/workflows/` is a GitHub Actions workflow.
        assert_eq!(
            FileLang::from_path(Path::new(".github/workflows/ci.yml")),
            FileLang::GithubActions
        );
        assert_eq!(
            FileLang::from_path(Path::new(".github/workflows/release.yaml")),
            FileLang::GithubActions
        );
        // Works with a project-prefixed path too (ancestry scan, not a prefix anchor).
        assert_eq!(
            FileLang::from_path(Path::new("repo/.github/workflows/build.yml")),
            FileLang::GithubActions
        );
        // A `.yml`/`.yaml` NOT under `.github/workflows/` stays plain YAML.
        assert_eq!(
            FileLang::from_path(Path::new("config/ci.yml")),
            FileLang::Yaml
        );
        // `.github/` but NOT in `workflows/` (e.g. dependabot config) stays YAML.
        assert_eq!(
            FileLang::from_path(Path::new(".github/dependabot.yml")),
            FileLang::Yaml
        );
        // A non-YAML file under `.github/workflows/` is not a workflow (e.g. a README).
        assert_eq!(
            FileLang::from_path(Path::new(".github/workflows/README.md")),
            FileLang::Other
        );
    }
}
