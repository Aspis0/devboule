//! Import-graph resolution + strongly-connected component detection.
//!
//! **PATH-HEURISTIC resolution, NOT name binding.** The resolver guesses which
//! file a specifier points to from path extensions and conventions, without a
//! type checker or scope analysis. Documented misses:
//!
//! - TS/JS: tsconfig path aliases, re-export chains (a re-exported symbol creates
//!   no edge because the exporting file isn't resolved as a target of the
//!   consumer's import).
//! - Rust: macro-generated `use` (proc-macros that expand to use statements are
//!   invisible at the AST level), multi-crate workspaces (crate-root detection
//!   uses a single `src/` heuristic).
//! - Python: namespace packages (PEP 420 — a directory without `__init__.py` is
//!   invisible to the `__init__.py` probe), editable installs.
//! - Go: no `go.mod` knowledge — module prefix stripping uses suffix-matching of
//!   known directories, so a deeply-nested module may not resolve.
//! - Kotlin: only `src/main/kotlin` / `src/test/kotlin` / `src/commonMain/kotlin`
//!   source roots are probed; custom Gradle source sets are invisible.
//! - C++: only quoted `#include "..."` (project-local); system `<...>` includes
//!   are skipped.
//!
//! Under-reporting is accepted and documented in the module header. The CKG and
//! Polis roads both consume `resolve_import_edges`; the Polis entry point
//! `import_graph` walks + parses + resolves in one call.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::backend::censor::detect::FileLang;
use crate::backend::structure;

/// A resolved import edge: from one source file to another, with an aggregate
/// symbol count (weight ≥ 1).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportEdge {
    pub from: String,
    pub to: String,
    pub weight: u32,
}

/// The import graph: resolved edges plus a cap flag from the walk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportGraph {
    pub edges: Vec<ImportEdge>,
    pub capped: bool,
    /// Every rel path the structure walk PARSED (i.e. every `FileFacts.path`).
    /// A file in this set has authoritative AST import data (possibly zero
    /// imports); a file NOT in it (no grammar / walk skipped / capped) falls
    /// back to regex extraction.
    pub files: BTreeSet<String>,
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

pub(crate) fn resolve_import_edges(
    facts: &[structure::FileFacts],
) -> Vec<ImportEdge> {
    let path_index: BTreeMap<&str, ()> = facts
        .iter()
        .map(|f| (f.path.as_str(), ()))
        .collect();

    let mut raw_edges: Vec<(String, String, u32)> = Vec::new();

    for fact in facts {
        let importer = &fact.path;
        let importer_dir = parent_dir(importer);

        for imp in &fact.imports {
            let resolved = match fact.lang {
                FileLang::Rust => resolve_rust(
                    &imp.specifier, importer, &importer_dir, fact, &path_index,
                ),
                FileLang::Ts => resolve_ts(
                    &imp.specifier, &importer_dir, &path_index,
                ),
                FileLang::Py => resolve_py(
                    &imp.specifier, importer, &importer_dir, &path_index,
                ),
                FileLang::Go => resolve_go(
                    &imp.specifier, &path_index,
                ),
                FileLang::Cpp => resolve_cpp(
                    &imp.specifier, &importer_dir, &path_index,
                ),
                FileLang::Kotlin => resolve_kotlin(
                    &imp.specifier, &path_index,
                ),
                _ => None,
            };

            if let Some(target) = resolved {
                if target == *importer {
                    continue;
                }
                let w = u32::max(imp.symbol_count, 1);
                raw_edges.push((importer.clone(), target, w));
            }
        }
    }

    let mut agg: HashMap<(String, String), u32> = HashMap::new();
    for (from, to, w) in raw_edges {
        *agg.entry((from, to)).or_default() += w;
    }

    let mut edges: Vec<ImportEdge> = agg
        .into_iter()
        .map(|((from, to), weight)| ImportEdge { from, to, weight })
        .collect();
    edges.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
    edges
}

// ===========================================================================
// Signature-based import-graph cache (P2.2 item 4)
// ===========================================================================

use std::sync::Mutex;

static IMPORT_GRAPH_CACHE: Mutex<Option<(std::path::PathBuf, u64, ImportGraph)>> =
    Mutex::new(None);

fn compute_tree_signature(root: &std::path::Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let walker = crate::backend::structure::make_walker(root);
    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map_or(false, |t| t.is_file()) {
            continue;
        }
        let rel = match entry.path().strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta
            .modified()
            .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64)
            .unwrap_or(0);
        let len = meta.len();
        rel.hash(&mut hasher);
        mtime.hash(&mut hasher);
        len.hash(&mut hasher);
    }
    hasher.finish()
}

pub fn import_graph_cached(root: &std::path::Path) -> Result<ImportGraph, String> {
    let canon = root
        .canonicalize()
        .map_err(|e| format!("import_graph: cannot canonicalize root: {e}"))?;
    let sig = compute_tree_signature(&canon);

    {
        let cache = IMPORT_GRAPH_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((ref cached_root, cached_sig, ref graph)) = *cache {
            if *cached_root == canon && cached_sig == sig {
                return Ok(graph.clone());
            }
        }
    }

    let graph = import_graph(&canon)?;

    {
        let mut cache = IMPORT_GRAPH_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        *cache = Some((canon, sig, graph.clone()));
    }

    Ok(graph)
}

pub fn import_graph(project_root: &Path) -> Result<ImportGraph, String> {
    if !project_root.is_dir() {
        return Err(format!(
            "graph: project root is not a directory: {}",
            project_root.display()
        ));
    }
    let scan = structure::collect_files(project_root);
    let files: BTreeSet<String> = scan.facts.iter()
        .map(|f| f.path.clone())
        .collect();
    let edges = resolve_import_edges(&scan.facts);
    Ok(ImportGraph {
        edges,
        capped: scan.capped,
        files,
    })
}

// ---------------------------------------------------------------------------
// Per-language resolution helpers
// ---------------------------------------------------------------------------

fn parent_dir(rel: &str) -> String {
    match rel.rfind('/') {
        Some(pos) => rel[..pos].to_string(),
        None => String::new(),
    }
}

fn join_path(dir: &str, name: &str) -> String {
    let raw = if dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", dir, name)
    };
    normalize_path(&raw)
}

fn normalize_path(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => { out.pop(); }
            _ => out.push(seg),
        }
    }
    out.join("/")
}

fn path_exists<'a>(key: &str, idx: &BTreeMap<&'a str, ()>) -> bool {
    idx.contains_key(key)
}

// --- Rust resolution ---

fn resolve_rust(
    spec: &str,
    importer: &str,
    importer_dir: &str,
    _fact: &structure::FileFacts,
    idx: &BTreeMap<&str, ()>,
) -> Option<String> {
    let crate_src = find_crate_src(importer);

    if let Some(rest) = spec.strip_prefix("crate::") {
        let rel = rest.replace("::", "/");
        for candidate in &[format!("{}.rs", rel), rel.clone(), format!("{}/mod.rs", rel)] {
            if let Some(full) = crate_src.as_ref().and_then(|s| {
                let joined = join_path(s, candidate);
                path_exists(&joined, idx).then_some(joined)
            }) {
                return Some(full);
            }
        }
        if let Some(last_slash) = rel.rfind('/') {
            let parent = &rel[..last_slash];
            let parent_path = format!("{}.rs", parent);
            if let Some(full) = crate_src.as_ref().and_then(|s| {
                let joined = join_path(s, &parent_path);
                path_exists(&joined, idx).then_some(joined)
            }) {
                return Some(full);
            }
            let parent_mod = format!("{}/mod.rs", parent);
            if let Some(full) = crate_src.as_ref().and_then(|s| {
                let joined = join_path(s, &parent_mod);
                path_exists(&joined, idx).then_some(joined)
            }) {
                return Some(full);
            }
        }
        return None;
    }

    if let Some(rest) = spec.strip_prefix("super::") {
        return resolve_rust_path(rest, importer_dir, idx);
    }

    if let Some(rest) = spec.strip_prefix("self::") {
        return resolve_rust_path(rest, importer_dir, idx);
    }

    if let Some(result) = resolve_rust_path(spec, importer_dir, idx) {
        return Some(result);
    }

    None
}

fn resolve_rust_path(
    spec: &str,
    base_dir: &str,
    idx: &BTreeMap<&str, ()>,
) -> Option<String> {
    let rel = spec.replace("::", "/");
    for candidate in &[format!("{}.rs", rel), rel.clone(), format!("{}/mod.rs", rel)] {
        let full = join_path(base_dir, candidate);
        if path_exists(&full, idx) {
            return Some(full);
        }
    }

    if let Some(last_slash) = rel.rfind('/') {
        let parent = &rel[..last_slash];
        for candidate in &[format!("{}.rs", parent), format!("{}/mod.rs", parent)] {
            let full = join_path(base_dir, candidate);
            if path_exists(&full, idx) {
                return Some(full);
            }
        }
    }
    None
}

fn find_crate_src(importer: &str) -> Option<String> {
    let segments: Vec<&str> = importer.split('/').collect();
    for i in (0..segments.len()).rev() {
        if segments[i] == "src" {
            return Some(segments[..=i].join("/"));
        }
    }
    if !importer.contains('/') {
        return Some("src".to_string());
    }
    let dir = parent_dir(importer);
    Some(dir)
}

// --- TS/JS resolution ---

const TS_EXTENSIONS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mts", ".cjs"];
const TS_INDEX_FILES: &[&str] = &["index.ts", "index.tsx", "index.js", "index.jsx"];

fn resolve_ts(
    spec: &str,
    importer_dir: &str,
    idx: &BTreeMap<&str, ()>,
) -> Option<String> {
    if spec.starts_with("./") || spec.starts_with("../") {
        let joined = join_path(importer_dir, spec);
        if path_exists(&joined, idx) {
            return Some(joined);
        }
        for ext in TS_EXTENSIONS {
            let candidate = format!("{}{}", joined, ext);
            if path_exists(&candidate, idx) {
                return Some(candidate);
            }
        }
        for idx_file in TS_INDEX_FILES {
            let candidate = format!("{}/{}", joined, idx_file);
            if path_exists(&candidate, idx) {
                return Some(candidate);
            }
        }
    }
    None
}

// --- Python resolution ---

fn resolve_py(
    spec: &str,
    importer: &str,
    importer_dir: &str,
    idx: &BTreeMap<&str, ()>,
) -> Option<String> {
    if spec.starts_with('.') {
        let dot_count = spec.chars().take_while(|c| *c == '.').count();
        let rest = &spec[dot_count..];
        let mut target_dir = importer_dir.to_string();
        for _ in 1..dot_count {
            target_dir = parent_dir(&target_dir);
        }
        let resolved = if rest.is_empty() {
            target_dir
        } else {
            join_path(&target_dir, &rest.replace('.', "/"))
        };
        return try_py_path(&resolved, idx);
    }

    let path_form = spec.replace('.', "/");
    if let Some(found) = try_py_path(&path_form, idx) {
        return Some(found);
    }
    if let Some(first_slash) = importer.find('/') {
        let pkg_dir = &importer[..first_slash];
        let pkg_path = join_path(pkg_dir, &path_form);
        if let Some(found) = try_py_path(&pkg_path, idx) {
            return Some(found);
        }
    }
    None
}

fn try_py_path(base: &str, idx: &BTreeMap<&str, ()>) -> Option<String> {
    let py_file = format!("{}.py", base);
    if path_exists(&py_file, idx) {
        return Some(py_file);
    }
    let init_file = format!("{}/__init__.py", base);
    if path_exists(&init_file, idx) {
        return Some(init_file);
    }
    None
}

// --- Go resolution ---

fn resolve_go(
    spec: &str,
    idx: &BTreeMap<&str, ()>,
) -> Option<String> {
    // Go imports are already /-separated (no domain-qualified dots to convert).
    let path_form = spec;

    // Direct match
    if path_exists(&path_form, idx) {
        return Some(path_form.to_string());
    }

    // Suffix match: find all known paths that end with `/<path_form>`.
    // If MORE than one matches, return None — a miss is better than a
    // silently-wrong arbitrary edge.
    let suffix = format!("/{}", path_form);
    let mut matches: Vec<&str> = idx
        .keys()
        .copied()
        .filter(|k| k.ends_with(&suffix) || *k == path_form)
        .collect();
    if matches.len() == 1 {
        return Some(matches[0].to_string());
    }
    None
}

// --- C/C++ resolution ---

fn resolve_cpp(
    spec: &str,
    importer_dir: &str,
    idx: &BTreeMap<&str, ()>,
) -> Option<String> {
    let joined = join_path(importer_dir, spec);
    if path_exists(&joined, idx) {
        return Some(joined);
    }
    // Suffix match: find all known paths ending in the spec filename.
    // If MORE than one matches, return None — a miss is better than a
    // silently-wrong arbitrary edge.
    let matches: Vec<&str> = idx
        .keys()
        .copied()
        .filter(|k| {
            k.ends_with(spec)
                && (k.len() == spec.len()
                    || k.as_bytes()[k.len() - spec.len() - 1] == b'/')
        })
        .collect();
    if matches.len() == 1 {
        return Some(matches[0].to_string());
    }
    None
}

// --- Kotlin resolution ---

const KOTLIN_SOURCE_ROOTS: &[&str] = &[
    "src/main/kotlin",
    "src/test/kotlin",
    "src/commonMain/kotlin",
];

fn resolve_kotlin(
    spec: &str,
    idx: &BTreeMap<&str, ()>,
) -> Option<String> {
    let path_form = spec.replace('.', "/");
    let file_form = format!("{}.kt", path_form);

    for root in KOTLIN_SOURCE_ROOTS {
        let candidate = format!("{}/{}", root, file_form);
        if path_exists(&candidate, idx) {
            return Some(candidate);
        }
    }

    // Try suffix-matching against known paths for multi-module projects
    for (&known, _) in idx {
        if known.ends_with(&format!("/{}", file_form)) {
            return Some(known.to_string());
        }
    }
    None
}

// ===========================================================================
// Iterative Tarjan SCC (P2.2 — shared by graph tests + sins dep-cycle)
// ===========================================================================

pub fn tarjan_scc(edges: &[ImportEdge]) -> Vec<Vec<String>> {
    if edges.is_empty() {
        return vec![];
    }

    let mut node_set: BTreeSet<&str> = BTreeSet::new();
    for e in edges {
        node_set.insert(&e.from);
        node_set.insert(&e.to);
    }
    let nodes: Vec<&str> = node_set.into_iter().collect();
    let node_to_idx: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i))
        .collect();

    let n = nodes.len();

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        if let (Some(&a), Some(&b)) = (node_to_idx.get(e.from.as_str()), node_to_idx.get(e.to.as_str())) {
            adj[a].push(b);
        }
    }
    for list in &mut adj {
        list.sort();
        list.dedup();
    }

    let mut index: Vec<i32> = vec![-1; n];
    let mut lowlink: Vec<i32> = vec![-1; n];
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut tarjan_stack: Vec<usize> = Vec::new();
    let mut idx_counter: i32 = 0;
    let mut sccs: Vec<Vec<String>> = Vec::new();

    let mut dfs_stack: Vec<(usize, usize)> = Vec::new();

    for start in 0..n {
        if index[start] != -1 {
            continue;
        }

        index[start] = idx_counter;
        lowlink[start] = idx_counter;
        idx_counter += 1;
        tarjan_stack.push(start);
        on_stack[start] = true;
        dfs_stack.push((start, 0));

        while let Some((v, ni)) = dfs_stack.last_mut() {
            let v = *v;
            if *ni < adj[v].len() {
                let w = adj[v][*ni];
                *ni += 1;
                if index[w] == -1 {
                    index[w] = idx_counter;
                    lowlink[w] = idx_counter;
                    idx_counter += 1;
                    tarjan_stack.push(w);
                    on_stack[w] = true;
                    dfs_stack.push((w, 0));
                } else if on_stack[w] {
                    lowlink[v] = lowlink[v].min(index[w]);
                }
            } else {
                dfs_stack.pop();
                if let Some((parent, _)) = dfs_stack.last() {
                    let p = parent;
                    lowlink[*p] = lowlink[*p].min(lowlink[v]);
                }
                if lowlink[v] == index[v] {
                    let mut scc: Vec<String> = Vec::new();
                    loop {
                        let w = tarjan_stack.pop().unwrap();
                        on_stack[w] = false;
                        scc.push(nodes[w].to_string());
                        if w == v {
                            break;
                        }
                    }
                    scc.sort();
                    sccs.push(scc);
                }
            }
        }
    }

    sccs.sort_by(|a, b| a.first().cmp(&b.first()));
    sccs
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::extract::RawImport;

    fn fact(path: &str, lang: FileLang, imports: Vec<RawImport>) -> structure::FileFacts {
        use std::collections::BTreeSet;
        structure::FileFacts {
            path: path.to_string(),
            lang,
            defined: BTreeSet::new(),
            identifiers: BTreeSet::new(),
            total_lines: 1,
            imports,
            items: Vec::new(),
        }
    }

    fn ri(spec: &str, count: u32) -> RawImport {
        RawImport {
            specifier: spec.to_string(),
            symbol_count: count,
        }
    }

    fn has_edge(edges: &[ImportEdge], from: &str, to: &str) -> Option<u32> {
        edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .map(|e| e.weight)
    }

    #[test]
    fn ts_relative_resolves_with_extension_probing() {
        let facts = vec![
            fact("src/index.ts", FileLang::Ts, vec![ri("./utils/helper", 2)]),
            fact("src/utils/helper.ts", FileLang::Ts, vec![]),
            fact("src/utils/helper.js", FileLang::Ts, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        let w = has_edge(&edges, "src/index.ts", "src/utils/helper.ts");
        assert!(w.is_some(), "expected edge to helper.ts, got {:?}", edges);
        assert!(w.unwrap() >= 2);
    }

    #[test]
    fn ts_relative_resolves_index_under_dir() {
        let facts = vec![
            fact("src/app.ts", FileLang::Ts, vec![ri("./components", 0)]),
            fact("src/components/index.ts", FileLang::Ts, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(has_edge(&edges, "src/app.ts", "src/components/index.ts").is_some());
    }

    #[test]
    fn ts_bare_package_skipped() {
        let facts = vec![fact("src/app.ts", FileLang::Ts, vec![ri("react", 1)])];
        let edges = resolve_import_edges(&facts);
        assert!(edges.is_empty(), "bare package should be skipped");
    }

    #[test]
    fn py_relative_import() {
        let facts = vec![
            fact("pkg/sub/module.py", FileLang::Py, vec![ri("..util", 1)]),
            fact("pkg/util.py", FileLang::Py, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(has_edge(&edges, "pkg/sub/module.py", "pkg/util.py").is_some());
    }

    #[test]
    fn py_absolute_dotted_from_root() {
        let facts = vec![
            fact("main.py", FileLang::Py, vec![ri("pkg.core", 0)]),
            fact("pkg/core.py", FileLang::Py, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(has_edge(&edges, "main.py", "pkg/core.py").is_some());
    }

    #[test]
    fn py_init_package() {
        let facts = vec![
            fact("app.py", FileLang::Py, vec![ri("pkg", 0)]),
            fact("pkg/__init__.py", FileLang::Py, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(has_edge(&edges, "app.py", "pkg/__init__.py").is_some());
    }

    #[test]
    fn rust_crate_resolves() {
        let facts = vec![
            fact("src/main.rs", FileLang::Rust, vec![ri("crate::polis::augure", 0)]),
            fact("src/polis/augure.rs", FileLang::Rust, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(has_edge(&edges, "src/main.rs", "src/polis/augure.rs").is_some());
    }

    #[test]
    fn rust_crate_mod_rs() {
        let facts = vec![
            fact("src/lib.rs", FileLang::Rust, vec![ri("crate::db::conn", 0)]),
            fact("src/db/conn/mod.rs", FileLang::Rust, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(has_edge(&edges, "src/lib.rs", "src/db/conn/mod.rs").is_some());
    }

    #[test]
    fn rust_super_resolves() {
        let facts = vec![
            fact("src/foo/bar.rs", FileLang::Rust, vec![ri("super::baz", 0)]),
            fact("src/foo/baz.rs", FileLang::Rust, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(has_edge(&edges, "src/foo/bar.rs", "src/foo/baz.rs").is_some());
    }

    #[test]
    fn rust_trailing_symbol_fallback() {
        let facts = vec![
            fact("src/main.rs", FileLang::Rust, vec![ri("crate::db::models::User", 0)]),
            fact("src/db/models.rs", FileLang::Rust, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(has_edge(&edges, "src/main.rs", "src/db/models.rs").is_some());
    }

    #[test]
    fn rust_external_crate_skipped() {
        let facts = vec![
            fact("src/main.rs", FileLang::Rust, vec![ri("serde::Serialize", 0)]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(edges.is_empty(), "external crate should be skipped");
    }

    #[test]
    fn cpp_quoted_include() {
        let facts = vec![
            fact("src/main.cpp", FileLang::Cpp, vec![ri("util/helpers.h", 0)]),
            fact("src/util/helpers.h", FileLang::Cpp, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(has_edge(&edges, "src/main.cpp", "src/util/helpers.h").is_some());
    }

    #[test]
    fn go_suffix_ambiguous_returns_no_edge() {
        let facts = vec![
            fact("src/main.go", FileLang::Go, vec![ri("common/config.go", 0)]),
            fact("pkg/common/config.go", FileLang::Go, vec![]),
            fact("lib/common/config.go", FileLang::Go, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(edges.is_empty(), "ambiguous suffix must yield no edge, got {:?}", edges);
    }

    #[test]
    fn cpp_suffix_ambiguous_returns_no_edge() {
        // The importer is in src/core/ -- the joined path src/core/utils.h
        // does NOT exist, so only suffix matching fires. Two files match
        // the suffix `utils.h` -> ambiguous -> no edge.
        let facts = vec![
            fact("src/core/main.cpp", FileLang::Cpp, vec![ri("utils.h", 0)]),
            fact("src/utils.h", FileLang::Cpp, vec![]),
            fact("lib/utils.h", FileLang::Cpp, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(edges.is_empty(), "ambiguous cpp suffix must yield no edge, got {:?}", edges);
    }

    #[test]
    fn unresolved_creates_no_edge() {
        let facts = vec![
            fact("src/a.rs", FileLang::Rust, vec![ri("crate::nonexistent::Module", 0)]),
        ];
        let edges = resolve_import_edges(&facts);
        assert!(edges.is_empty());
    }

    #[test]
    fn weight_aggregation() {
        let facts = vec![
            fact("a.rs", FileLang::Rust, vec![ri("crate::b", 3), ri("crate::b", 2)]),
            fact("src/b.rs", FileLang::Rust, vec![]),
        ];
        let edges = resolve_import_edges(&facts);
        let w = has_edge(&edges, "a.rs", "src/b.rs");
        assert_eq!(w, Some(5), "weights should sum: 3+2=5");
    }

    #[test]
    fn edge_output_is_deterministic() {
        let facts = vec![
            fact("z.rs", FileLang::Rust, vec![ri("crate::a", 1)]),
            fact("a.rs", FileLang::Rust, vec![ri("crate::b", 1)]),
            fact("src/a.rs", FileLang::Rust, vec![]),
            fact("src/b.rs", FileLang::Rust, vec![]),
        ];
        let e1 = resolve_import_edges(&facts);
        let e2 = resolve_import_edges(&facts);
        assert_eq!(e1, e2, "same input must produce identical edges");
    }

    fn edge(from: &str, to: &str) -> ImportEdge {
        ImportEdge {
            from: from.to_string(),
            to: to.to_string(),
            weight: 1,
        }
    }

    #[test]
    fn tarjan_two_node_cycle() {
        let edges = vec![edge("a.rs", "b.rs"), edge("b.rs", "a.rs")];
        let sccs = tarjan_scc(&edges);
        assert_eq!(sccs.len(), 1, "one SCC expected");
        assert_eq!(sccs[0], vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn tarjan_three_node_cycle_plus_tail() {
        let edges = vec![
            edge("a.rs", "b.rs"),
            edge("b.rs", "c.rs"),
            edge("c.rs", "a.rs"),
            edge("d.rs", "a.rs"),
        ];
        let sccs = tarjan_scc(&edges);
        // Find the non-trivial SCC: a-b-c cycle.
        let cycle: Vec<&Vec<String>> = sccs.iter().filter(|s| s.len() >= 2).collect();
        assert_eq!(cycle.len(), 1, "exactly one multi-node SCC");
        assert_eq!(cycle[0], &vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()]);
    }

    #[test]
    fn tarjan_no_cycle_returns_empty() {
        let edges = vec![edge("a.rs", "b.rs"), edge("b.rs", "c.rs")];
        let sccs = tarjan_scc(&edges);
        // No multi-node SCC — only singletons (which the dep-cycle caller filters).
        assert!(sccs.iter().all(|s| s.len() == 1), "no multi-node cycles");
    }

    #[test]
    fn tarjan_self_loop_is_scc() {
        let edges = vec![edge("a.rs", "a.rs")];
        let sccs = tarjan_scc(&edges);
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0], vec!["a.rs"]);
    }

    #[test]
    fn tarjan_deterministic_with_shuffled_input() {
        let edges = vec![
            edge("z.rs", "a.rs"),
            edge("a.rs", "b.rs"),
            edge("b.rs", "z.rs"),
        ];
        let s1 = tarjan_scc(&edges);
        let mut rev = edges.clone();
        rev.reverse();
        let s2 = tarjan_scc(&rev);
        assert_eq!(s1, s2, "output must be deterministic regardless of input order");
    }

    #[test]
    fn tarjan_deep_chain_no_stack_overflow() {
        let n = 5000usize;
        let mut edges = Vec::with_capacity(n - 1);
        for i in 0..n - 1 {
            edges.push(edge(
                &format!("file_{}.rs", i),
                &format!("file_{}.rs", i + 1),
            ));
        }
        let sccs = tarjan_scc(&edges);
        // Deep chain: no multi-node SCCs; all singletons, no stack overflow.
        assert!(sccs.iter().all(|s| s.len() == 1), "deep chain must not overflow and must have no cycles");
    }
}
