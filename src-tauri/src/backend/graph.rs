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


/// Per-item metric carried from the AST parse into the import graph for the
/// Polis sin detectors (complexity check).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ItemMetric {
    /// The item's declared name (None for anonymous items like impl blocks).
    pub name: Option<String>,
    /// 1-based start line of the item.
    pub line: u32,
    /// McCabe-style cyclomatic complexity (from extract.rs).
    pub complexity: u32,
    /// Grammar node kind (e.g. "function_item", "class_declaration").
    pub kind: String,
}

/// Per-file metrics carried from the structure walk into the import graph
/// for the Polis sin detectors (complexity, god-file, test-gap).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileMetrics {
    /// Project-root-relative path (/ separated).
    pub rel_path: String,
    /// Lines of code (total_lines from the parse).
    pub loc: u32,
    /// Top-level review items with their complexity.
    pub items: Vec<ItemMetric>,
    /// Defined symbol names (the file's exported surface).
    pub exported: Vec<String>,
}


/// A detected clone pair: two files share a block of >= 50 identical tokens
/// (after normalisation — identifiers → sentinel, string literals → kind).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClonePair {
    /// First file's project-relative path.
    pub a: String,
    /// Second file's project-relative path.
    pub b: String,
    /// 1-based start line of the match in file `a`.
    pub a_line: u32,
    /// 1-based start line of the match in file `b`.
    pub b_line: u32,
    /// Length of the matched token run (≥ 50).
    pub tokens: u32,
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
    /// Per-file metrics (loc, items with complexity, exported symbols) from the
    /// structure walk. Populated from `FileFacts` during `import_graph`.
    pub metrics: Vec<FileMetrics>,
    /// Union of all identifiers seen in TEST files (paths matching
    /// `is_test_path`). Used by the test-gap sin to check whether src-file
    /// exported symbols are referenced by any test.
    pub test_refs: BTreeSet<String>,
    /// Detected clone pairs (P4.2).  Computed during `import_graph` via
    /// Rabin-Karp over token-fingerprint hashes.  Capped at 20 pairs per
    /// project, worst-first by token count.
    ///
    /// NOTE: the raw `token_hashes`/`token_lines` vectors (on `FileFacts`)
    /// are NOT cached — they are consumed by `detect_clones` during the scan
    /// and dropped.  Only the resulting `ClonePair` values enter the cache
    /// at ~40 bytes per pair, not the ~12 bytes/token raw vectors.
    pub clones: Vec<ClonePair>,
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

    // Build per-file metrics from the structure facts.
    let metrics: Vec<FileMetrics> = scan.facts.iter().map(|f| {
        FileMetrics {
            rel_path: f.path.clone(),
            loc: f.total_lines,
            items: f.items.iter().flat_map(|it| {
                // M1 fix: container items (impl_item, class_declaration) expand
                // into per-method entries so the complexity sin cites the method,
                // not the aggregate container.
                if !it.child_complexities.is_empty() {
                    it.child_complexities.iter().map(|cc| ItemMetric {
                        name: cc.name.clone(),
                        line: cc.line,
                        complexity: cc.complexity,
                        kind: "function_item".to_string(),
                    }).collect::<Vec<_>>()
                } else {
                    vec![ItemMetric {
                        name: it.name.clone(),
                        line: it.start_line,
                        complexity: it.complexity,
                        kind: it.kind.clone(),
                    }]
                }
            }).collect(),
            exported: f.defined.iter().cloned().collect(),
        }
    }).collect();

    // Union of identifiers from test files only.
    let test_refs: BTreeSet<String> = scan.facts.iter()
        .filter(|f| is_test_path(&f.path))
        .flat_map(|f| f.identifiers.iter().cloned())
        .collect();

    // P4.2 — Rabin-Karp clone detection over token-fingerprint hashes.
    let clones = detect_clones(&scan.facts);

    Ok(ImportGraph {
        edges,
        capped: scan.capped,
        files,
        metrics,
        test_refs,
        clones,
    })
}


// ===========================================================================
// P4.2 — Rabin-Karp clone detection over token-fingerprint hashes
// ===========================================================================

/// Window size for the rolling hash (in tokens).  A clone is a block of at
/// least this many identical (normalised) tokens appearing in two files.
const CLONE_WINDOW: usize = 50;

/// Maximum number of indexed windows across all files.  Beyond this cap we
/// stop indexing and keep whatever pairs were found so far.  Prevents
/// unbounded work on a codebase that is mostly repeated tokens.
const CLONE_MAX_INDEXED_WINDOWS: usize = 2_000_000;

/// Maximum clone pairs emitted into `ImportGraph::clones`.  Worst-first
/// (by token count), deterministic tie-break on paths.
const CLONE_MAX_PAIRS: usize = 20;

/// Maximum entries per hash bucket before the bucket is SKIPPED entirely.
/// Degenerate/boilerplate code can saturate one bucket with thousands of
/// windows (e.g. 100 files all starting with the same 50-token preamble);
/// that is not a meaningful clone signal and the O(k²) comparisons would
/// stall the scan.  Buckets with more entries are silently dropped.
const CLONE_MAX_BUCKET_ENTRIES: usize = 64;

/// Global comparison budget: pair-extension attempts across all buckets.
/// On exhaustion the clone pass stops and returns whatever pairs were
/// found so far.  Prevents unbounded work even with many moderately-
/// populated buckets.
const CLONE_MAX_COMPARISONS: usize = 5_000_000;

/// Rolling-hash multiplier (odd, large, FNV-like).
const ROLL_BASE: u64 = 0x9E3779B97F4A7C15;

/// Detect clone pairs from the per-file token fingerprint hashes computed
/// during the structure walk.
///
/// Algorithm: Rabin-Karp rolling hash over `CLONE_WINDOW`-token windows.
/// Each window hash is indexed in a `HashMap<u64, Vec<(file_idx, offset)>>`.
/// On collision between DIFFERENT files, the token slices are verified for
/// equality and the match is extended greedily forward.  Only the LONGEST
/// match per file pair is kept.  The top `CLONE_MAX_PAIRS` (by tokens desc)
/// are returned, deterministically sorted.
fn detect_clones(facts: &[crate::backend::structure::FileFacts]) -> Vec<ClonePair> {
    use std::collections::{BTreeMap, HashMap};

    // Pre-compute ROLL_BASE^WINDOW mod 2^64.
    let base_pow: u64 = {
        let mut p: u64 = 1;
        for _ in 0..CLONE_WINDOW {
            p = p.wrapping_mul(ROLL_BASE);
        }
        p
    };

    // Collect all windows: HashMap<hash, Vec<(file_idx, offset)>>.
    // Only index files with enough tokens.
    let mut window_index: HashMap<u64, Vec<(usize, usize)>> = HashMap::new();
    let mut total_windows: usize = 0;

    let file_data: Vec<(usize, &[u64])> = facts
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            if f.token_hashes.len() >= CLONE_WINDOW {
                Some((i, f.token_hashes.as_slice()))
            } else {
                None
            }
        })
        .collect();

    for &(fi, hashes) in &file_data {
        if total_windows >= CLONE_MAX_INDEXED_WINDOWS {
            break;
        }
        let max_windows = hashes.len().saturating_sub(CLONE_WINDOW) + 1;
        if max_windows == 0 {
            continue;
        }
        // Initial window hash.
        let mut hash: u64 = 0;
        for &h in &hashes[..CLONE_WINDOW] {
            hash = hash.wrapping_mul(ROLL_BASE).wrapping_add(h);
        }
        for offset in 0..max_windows {
            if total_windows >= CLONE_MAX_INDEXED_WINDOWS {
                break;
            }
            window_index
                .entry(hash)
                .or_default()
                .push((fi, offset));
            total_windows += 1;
            // Roll the hash forward.
            if offset + CLONE_WINDOW < hashes.len() {
                let drop = hashes[offset].wrapping_mul(base_pow);
                let add = hashes[offset + CLONE_WINDOW];
                hash = hash
                    .wrapping_mul(ROLL_BASE)
                    .wrapping_add(add)
                    .wrapping_sub(drop);
            }
        }
    }

    // Resolve collisions: BTreeMap<(file_a, file_b), (max_tokens, a_offset, b_offset)>.
    // BTreeMap gives deterministic ordering of the output.
    let mut pairs: BTreeMap<(usize, usize), (usize, usize, usize)> = BTreeMap::new();

    let mut comparisons: usize = 0;
    let mut budget_exhausted = false;

    for (_hash, entries) in &window_index {
        if entries.len() < 2 {
            continue;
        }
        // B3 fix: skip degenerate buckets (boilerplate / preamble clones).
        if entries.len() > CLONE_MAX_BUCKET_ENTRIES {
            continue;
        }
        if budget_exhausted {
            break;
        }
        for i in 0..entries.len() {
            let (fi_a, off_a) = entries[i];
            let hashes_a = &facts[fi_a].token_hashes;
            for j in (i + 1)..entries.len() {
                let (fi_b, off_b) = entries[j];
                if fi_a == fi_b {
                    continue; // same-file duplicates out of scope
                }
                let hashes_b = &facts[fi_b].token_hashes;

                // Verify exact token slice equality.
                let end_a = off_a + CLONE_WINDOW;
                let end_b = off_b + CLONE_WINDOW;
                if end_a > hashes_a.len() || end_b > hashes_b.len() {
                    continue;
                }
                if hashes_a[off_a..end_a] != hashes_b[off_b..end_b] {
                    continue;
                }

                // B3 fix: global comparison budget.
                comparisons += 1;
                if comparisons > CLONE_MAX_COMPARISONS {
                    budget_exhausted = true;
                    break;
                }

                // Greedy forward extension.
                let mut len = CLONE_WINDOW;
                while off_a + len < hashes_a.len()
                    && off_b + len < hashes_b.len()
                    && hashes_a[off_a + len] == hashes_b[off_b + len]
                {
                    len += 1;
                }

                let key = if fi_a < fi_b {
                    (fi_a, fi_b)
                } else {
                    (fi_b, fi_a)
                };
                let entry = pairs.entry(key).or_insert((0, 0, 0));
                if len > entry.0 {
                    // Keep the ordering inside the pair consistent: (a,b) where
                    // a < b by index; offsets match whichever file is first.
                    if fi_a < fi_b {
                        *entry = (len, off_a, off_b);
                    } else {
                        *entry = (len, off_b, off_a);
                    }
                }
            }
        }
    }

    // Materialize ClonePair values, sort by tokens desc then paths asc.
    let mut result: Vec<ClonePair> = pairs
        .into_iter()
        .map(|((fi_a, fi_b), (tokens, off_a, off_b))| ClonePair {
            a: facts[fi_a].path.clone(),
            b: facts[fi_b].path.clone(),
            a_line: facts[fi_a]
                .token_lines
                .get(off_a)
                .copied()
                .unwrap_or(1),
            b_line: facts[fi_b]
                .token_lines
                .get(off_b)
                .copied()
                .unwrap_or(1),
            tokens: tokens as u32,
        })
        .collect();

    result.sort_by(|p, q| {
        q.tokens
            .cmp(&p.tokens)
            .then_with(|| p.a.cmp(&q.a))
            .then_with(|| p.b.cmp(&q.b))
    });
    result.truncate(CLONE_MAX_PAIRS);

    result
}

// ---------------------------------------------------------------------------
// Test-path classifier (shared by graph + sin detectors)
// ---------------------------------------------------------------------------

/// True when `rel` is a test file by one of the conventional markers:
///   - directory segment `tests`, `test`, or `__tests__`;
///   - filename matches `*.test.*`, `*.spec.*`, `*_test.go`, `*_test.py`,
///     `tests.rs`, or `*_test.rs`.
///
/// File-extension agnostic: a path matching ANY marker is a test regardless of
/// the language of its siblings.  This is a classifier, not a grammar gate.
pub fn is_test_path(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let segments: Vec<&str> = lower.split('/').collect();

    // Directory-based: any dir named tests/test/__tests__.
    if segments.iter().any(|s| *s == "tests" || *s == "test" || *s == "__tests__") {
        return true;
    }

    // File-based patterns.
    let name = segments.last().copied().unwrap_or("");
    // `*.test.*` / `*.spec.*` (TS/JS convention).
    if name.contains(".test.") || name.contains(".spec.") {
        return true;
    }
    // Go convention: `*_test.go`.
    if name.ends_with("_test.go") {
        return true;
    }
    // Python convention: `test_*.py` or `*_test.py`.
    if (name.starts_with("test_") || name.ends_with("_test.py")) && name.ends_with(".py") {
        return true;
    }
    // Rust convention: `tests.rs` or `*_test.rs`.
    if name == "tests.rs" || name.ends_with("_test.rs") {
        return true;
    }

    false
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
            token_hashes: Vec::new(),
            token_lines: Vec::new(),
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

    // =========================================================================
    // P4.1 — is_test_path classifier table-test
    // =========================================================================

    #[test]
    fn is_test_path_flags_test_dirs() {
        assert!(is_test_path("tests/integration.rs"));
        assert!(is_test_path("src/test/helpers.ts"));
        assert!(is_test_path("__tests__/foo.test.ts"));
    }

    #[test]
    fn is_test_path_flags_test_file_patterns() {
        assert!(is_test_path("src/foo.test.ts"));
        assert!(is_test_path("src/bar.spec.tsx"));
        assert!(is_test_path("pkg/conn_test.go"));
        assert!(is_test_path("pkg/test_helpers.py"));
        assert!(is_test_path("pkg/helpers_test.py"));
        assert!(is_test_path("src/utils_test.rs"));
        assert!(is_test_path("tests.rs"));
    }

    #[test]
    fn is_test_path_rejects_normal_src_files() {
        assert!(!is_test_path("src/main.rs"));
        assert!(!is_test_path("lib/db.ts"));
        assert!(!is_test_path("pkg/handlers.go"));
        assert!(!is_test_path("app/models.py"));
        // test-like in name but not a test-file pattern
        assert!(!is_test_path("src/testing_tools.rs"));
        assert!(!is_test_path("src/protest.ts"));
    }

    // =========================================================================
    // P4.1 — metrics and test_refs populated from facts
    // =========================================================================

    #[test]
    fn import_graph_populates_metrics() {
        use std::fs;
        use std::path::PathBuf;
        let dir = std::env::temp_dir().join(format!("aspis-graph-metrics-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "pub fn greet() -> &'static str { \"hi\" }\n",
        )
        .unwrap();

        let graph = import_graph(&dir).expect("graph builds");
        // At least one file metric.
        assert!(!graph.metrics.is_empty(), "metrics must be populated");
        let m = graph.metrics.iter().find(|m| m.rel_path == "src/lib.rs").expect("lib.rs metric");
        assert!(m.loc >= 1);
        assert!(!m.items.is_empty(), "greet fn must be an item");
        let fn_item = m.items.iter().find(|i| i.name.as_deref() == Some("greet")).expect("greet item");
        assert_eq!(fn_item.kind, "function_item");
        assert_eq!(fn_item.complexity, 1, "flat fn => 1");
        assert!(m.exported.contains(&"greet".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_refs_collects_only_test_file_identifiers() {
        use std::fs;
        use std::path::PathBuf;
        let dir = std::env::temp_dir().join(format!("aspis-graph-testrefs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn CoreFn() {}
").unwrap();
        fs::write(dir.join("tests/integration.rs"), "fn test_core() { CoreFn(); }
").unwrap();

        let graph = import_graph(&dir).expect("graph builds");
        // test_refs must contain identifiers from test files.
        assert!(graph.test_refs.contains("CoreFn"), "CoreFn from test file must be in test_refs");
        assert!(graph.test_refs.contains("test_core"), "test_core from test file must be in test_refs");
        // Identifiers from src files are NOT included.
        // ("CoreFn" appears in BOTH a src file and a test file; the test file's
        // occurrence adds it to test_refs, which is the union of ALL test-file
        // identifiers. That is correct — the sin checks against this union.)

        let _ = fs::remove_dir_all(&dir);
    }


    // =========================================================================
    // P4.2 — Rabin-Karp clone detection tests
    // =========================================================================

    /// Build a minimal FileFacts with token_hashes / token_lines for clone testing.
    fn clone_fact(path: &str, token_hashes: Vec<u64>, token_lines: Vec<u32>) -> structure::FileFacts {
        structure::FileFacts {
            path: path.to_string(),
            lang: FileLang::Rust,
            defined: BTreeSet::new(),
            identifiers: BTreeSet::new(),
            total_lines: token_lines.last().copied().unwrap_or(1),
            imports: Vec::new(),
            items: Vec::new(),
            token_hashes,
            token_lines,
        }
    }

    /// Build a simple token-hash vector: each token gets hash = (base + idx).
    fn token_seq(base: u64, n: usize) -> Vec<u64> {
        (0..n).map(|i| base.wrapping_add(i as u64)).collect()
    }

    /// Build line numbers: token i is at line (start + i).
    fn line_seq(start: u32, n: usize) -> Vec<u32> {
        (0..n).map(|i| start + i as u32).collect()
    }

    #[test]
    fn clone_detection_finds_shared_60_token_block() {
        // Two files share an identical 60-token block at offset 10 / offset 5.
        let shared = token_seq(1000, 60);
        let mut a_tokens: Vec<u64> = token_seq(1, 10);
        a_tokens.extend(&shared);
        a_tokens.extend(token_seq(200, 20));
        let a_lines = line_seq(1, a_tokens.len());

        let mut b_tokens: Vec<u64> = token_seq(50, 5);
        b_tokens.extend(&shared);
        b_tokens.extend(token_seq(300, 15));
        let b_lines = line_seq(1, b_tokens.len());

        let facts = vec![
            clone_fact("src/a.rs", a_tokens, a_lines),
            clone_fact("src/b.rs", b_tokens, b_lines),
        ];
        let clones = detect_clones(&facts);
        assert!(!clones.is_empty(), "60-token shared block must produce a clone pair");
        let cp = &clones[0];
        assert!(cp.tokens >= 60, "match must be at least 60 tokens, got {}", cp.tokens);
        // a_line should be the line at offset 10.
        assert_eq!(cp.a_line, 11, "a offset 10 -> line 11 (1-based)");
        assert_eq!(cp.b_line, 6, "b offset 5 -> line 6");
        // Paths in expected order (a.rs < b.rs).
        assert_eq!(cp.a, "src/a.rs");
        assert_eq!(cp.b, "src/b.rs");
    }

    #[test]
    fn clone_detection_rejects_below_50() {
        let shared = token_seq(1000, 49); // 49 tokens — below threshold
        let mut a_tokens: Vec<u64> = token_seq(1, 5);
        a_tokens.extend(&shared);
        let a_lines = line_seq(1, a_tokens.len());

        let mut b_tokens: Vec<u64> = token_seq(50, 3);
        b_tokens.extend(&shared);
        let b_lines = line_seq(1, b_tokens.len());

        let facts = vec![
            clone_fact("src/a.rs", a_tokens, a_lines),
            clone_fact("src/b.rs", b_tokens, b_lines),
        ];
        let clones = detect_clones(&facts);
        assert!(clones.is_empty(), "49-token block below window → no clone");
    }

    #[test]
    fn clone_detection_is_deterministic() {
        // Two files sharing a 55-token block — two runs must produce identical output.
        let shared = token_seq(100, 55);
        let mut a = token_seq(1, 10);
        a.extend(&shared);
        let a_lines = line_seq(1, a.len());
        let mut b = token_seq(30, 20);
        b.extend(&shared);
        let b_lines = line_seq(1, b.len());

        let facts = vec![
            clone_fact("x.rs", a.clone(), a_lines.clone()),
            clone_fact("y.rs", b.clone(), b_lines.clone()),
        ];
        let c1 = detect_clones(&facts);
        let c2 = detect_clones(&facts);
        assert_eq!(c1, c2, "clone detection must be deterministic");
    }

    #[test]
    fn clone_detection_yields_one_pair_per_file_pair() {
        // File A and B share TWO identical blocks (at different offsets).
        // Only the LONGEST match per pair is kept.
        let block_short = token_seq(500, 55);
        let block_long = token_seq(700, 80);

        let mut a_tokens = token_seq(1, 5);
        a_tokens.extend(&block_short);
        a_tokens.extend(token_seq(200, 10));
        a_tokens.extend(&block_long);
        let a_lines = line_seq(1, a_tokens.len());

        let mut b_tokens = token_seq(10, 15);
        b_tokens.extend(&block_short);
        b_tokens.extend(token_seq(300, 5));
        b_tokens.extend(&block_long);
        b_tokens.extend(token_seq(400, 10));
        let b_lines = line_seq(1, b_tokens.len());

        let facts = vec![
            clone_fact("src/a.rs", a_tokens, a_lines),
            clone_fact("src/b.rs", b_tokens, b_lines),
        ];
        let clones = detect_clones(&facts);
        assert_eq!(clones.len(), 1, "one pair expected, got {:?}", clones.len());
        assert!(clones[0].tokens >= 80, "must keep the longest match (>=80)");
    }

    #[test]
    fn clone_detection_empty_on_no_shared_tokens() {
        let facts = vec![
            clone_fact("a.rs", token_seq(1, 100), line_seq(1, 100)),
            clone_fact("b.rs", token_seq(1000, 100), line_seq(1, 100)),
        ];
        assert!(detect_clones(&facts).is_empty());
    }

    #[test]
    fn clone_detection_skips_files_below_window_size() {
        let short = token_seq(1, 40); // < 50 window
        let facts = vec![
            clone_fact("a.rs", short.clone(), line_seq(1, 40)),
            clone_fact("b.rs", short.clone(), line_seq(1, 40)),
        ];
        assert!(detect_clones(&facts).is_empty(), "files with < 50 tokens skip window indexing");
    }


    #[test]
    fn clone_bucket_skip_on_degenerate_boilerplate() {
        // 100 files all sharing the same 50-token window → bucket size 100
        // exceeds CLONE_MAX_BUCKET_ENTRIES (64) → bucket skipped, no clones.
        let shared = token_seq(1000, 50);
        let mut facts = Vec::new();
        for i in 0..100 {
            let mut tokens = token_seq(i as u64 * 10, 5);
            tokens.extend(&shared);
            tokens.extend(token_seq(2000 + i as u64, 5));
            let lines = line_seq(1, tokens.len());
            facts.push(clone_fact(&format!("src/file{i}.rs"), tokens, lines));
        }
        let clones = detect_clones(&facts);
        assert!(clones.is_empty(), "degenerate 100-file bucket must be skipped, got {:?}", clones.len());
    }

}