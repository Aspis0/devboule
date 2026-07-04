//! DETERMINISTIC cross-file STRUCTURE graph + architectural-"spine" ranking.
//!
//! This is the foundation of the planner (Phase 11): the planner's STRUCTURE phase
//! needs to know, BEFORE any LLM is involved, which files a project's architecture
//! hangs off of — the handful of modules that the rest of the code reaches into. We
//! compute that purely from the syntax: NO LLM, NO GPU, NO network. Same tree on disk
//! ⇒ byte-identical graph + spine ordering (the determinism contract, enforced by
//! sorting every collection so no `HashMap` iteration order ever leaks into the
//! output).
//!
//! REUSE (the load-bearing constraint): the per-file tree-sitter extraction ALREADY
//! EXISTS in [`crate::backend::censor::extract`] — `parse_file` gives us, per file, the
//! top-level review items (with names) and the whole-file identifier set; and
//! [`crate::backend::censor::detect::FileLang::from_path`] maps a path to a language.
//! This module adds NO second parser; it only builds a cross-file graph ON TOP of that
//! per-file substrate.
//!
//! THE EDGE HEURISTIC AND ITS LIMITS (read this before trusting an edge):
//! tree-sitter is SYNTAX-ONLY — there is no type checker, no import resolver, no scope
//! analysis here. So a cross-file "A references a symbol defined in B" edge is
//! NECESSARILY a NAME-RESOLUTION HEURISTIC, not a sound semantic reference:
//!   1. We build `symbol_name -> {defining files}` from every file's top-level defined
//!      names.
//!   2. For each file A, for each identifier A's grammar saw, if that identifier names a
//!      symbol DEFINED in some OTHER file B, we add a directed edge A -> B.
//! This OVER-CONNECTS in two ways we deliberately mitigate (and otherwise accept):
//!   - SHADOWING / shared short names: a local variable named like a far-away exported
//!     symbol creates a phantom edge. Mitigated by [`MIN_SYMBOL_LEN`] (ignore names too
//!     short to be a meaningful cross-file handle).
//!   - UBIQUITOUS names: a `new`/`init`/`Config` defined in dozens of files is not a
//!     useful architectural signal. Mitigated by [`MAX_DEFINING_FILES`] (a name defined
//!     in too many files is dropped as an edge source/target).
//! It also UNDER-CONNECTS: a symbol referenced only via a method call on a value whose
//! type lives elsewhere, or via a re-export, may produce no edge. This is acceptable —
//! the spine is a RANKING signal for the planner, not a call graph. The ranking is
//! robust to individual missing/spurious edges because it aggregates DISTINCT-referrer
//! counts across the whole tree.
//!
//! BOUNDED + NEVER PANICS: the walk has TWO hard bounds — at most [`MAX_FILES`] files are
//! PARSED and at most [`MAX_WALK_ENTRIES`] entries are CONSIDERED in total (so a tree full
//! of skipped-but-stat'd files still terminates); either bound sets
//! [`StructureGraph::capped`] so the planner knows the graph is partial. Files over
//! [`MAX_FILE_BYTES`] are skipped; unreadable / un-stat'able / non-UTF-8-path files are
//! skipped (not fatal) and counted in [`StructureGraph::skipped_unreadable`]; the only
//! error path is a project root that cannot be walked at all. No `unwrap` on IO.
//!
//! PER-LANGUAGE EDGE NOISE: the identifier set's signal-to-noise differs by language.
//! HTML is the worst offender — its "identifiers" are tag names + attribute values
//! (`div`, `src`, `id`, `login`, …), many >= [`MIN_SYMBOL_LEN`], which would collide with
//! code symbols and manufacture phantom edges. So HTML files are NEVER edge SOURCES (they
//! remain nodes and may still be edge TARGETS); they contribute no out-edges. Other
//! languages can still over-connect on shared short names, mitigated as described above.
//!
//! PRODUCTION CALLER (Phase 11.2): the builder is no longer "dark". The headless CLI
//! bridge [`run_structure_cli`] (invoked as `aspis-management structure --root <path>`,
//! detected in `main` before the GUI builder runs) calls [`build_structure_graph`] and
//! prints the [`StructureGraph`] as JSON to stdout, so the shared, read-only
//! `project_structure` MCP tool can reuse THIS builder (zero tree-sitter duplication) by
//! shelling out to the app binary. The pure graph logic is exercised by this module's
//! tests; the CLI bridge by [`tests::cli_*`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::backend::censor::detect::FileLang;
use crate::backend::censor::extract::{self, ReviewItem};

/// Hard cap on the number of source files PARSED into the graph. A graph over more than
/// a couple thousand files is both slow to build and rarely a single coherent "project"
/// the planner reasons about; we stop parsing here and report the cap was hit via
/// [`StructureGraph::capped`] (the caller sees the flag + the [`StructureGraph::scanned`]
/// count, never a silent truncation).
pub const MAX_FILES: usize = 2_000;

/// Hard cap on the TOTAL number of walk entries considered (parsed + skipped of every
/// kind), independent of [`MAX_FILES`]. Without this, a tree with a huge number of
/// skipped-but-stat'd files (e.g. a stray vendored JS tree that escapes [`SKIP_DIRS`] /
/// `.gitignore`) would walk unboundedly because skipped files never consume the parse
/// cap. Set well above `MAX_FILES` so a normal project never trips it; tripping it sets
/// [`StructureGraph::capped`]. This is the absolute upper bound on the walk's work.
pub const MAX_WALK_ENTRIES: usize = 50_000;

/// Skip any single file larger than this (512 KiB). A source file this large is almost
/// always generated/vendored/minified — parsing it is expensive and its identifiers
/// pollute the name map. Skipped files are counted in
/// [`StructureGraph::skipped_too_large`], never silently dropped.
pub const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Ignore defined symbol names shorter than this (in bytes) when building edges. Names
/// like `i`, `x`, `id`, `ok` collide across unrelated files and produce phantom edges;
/// a real cross-file architectural handle is virtually always >= 3 chars. This is the
/// primary false-positive mitigation for the syntax-only heuristic (see module header).
pub const MIN_SYMBOL_LEN: usize = 3;

/// A symbol name DEFINED in more than this many distinct files is too generic to be a
/// useful edge (e.g. `new`, `default`, `Config`, `Error` defined all over). Such names
/// are dropped from the name map entirely, so they create NO edges in either direction.
/// Tuned conservatively: most genuinely-central symbols are defined ONCE.
pub const MAX_DEFINING_FILES: usize = 5;

/// The maximum spine size. We rank files by in-degree and take the top [`SPINE_MAX`]
/// as the architectural spine — enough to capture a project's core without drowning the
/// planner in a long list. The spine may be SHORTER (or empty): only positive-in-degree
/// files qualify, so a tiny project produces fewer entries and we NEVER pad with
/// zero-in-degree files.
pub const SPINE_MAX: usize = 8;

/// How many of a spine file's most-referenced symbols to surface, as a hint to the
/// planner ("this file is central BECAUSE these names are reached for").
const TOP_REFERENCED_SYMBOLS: usize = 5;

/// Directory names that are ALWAYS skipped regardless of `.gitignore`, in addition to
/// the `ignore` crate's built-in hidden-file + `.gitignore` filtering. These are build
/// artifacts / dependency trees that are never part of the project's own architecture
/// and would blow the file cap with vendored code. `.git` is already skipped as a
/// hidden dir; the rest are NOT hidden so we must name them.
pub(crate) const SKIP_DIRS: [&str; 6] = ["target", "node_modules", "dist", "build", "out", ".git"];

/// One node in the structure graph: a single source file and its degree in the
/// cross-file reference graph. Serialized camelCase for the later MCP/JSON exposure +
/// the planner's STRUCTURE phase (Phase 11.2 wires the Tauri command; this module is the
/// pure builder).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    /// Project-root-relative path with `/` separators (stable across OSes — the on-disk
    /// separator is normalized so the same tree yields the same string on Windows and
    /// unix).
    pub path: String,
    /// The language the file was parsed as (the `FileLang` debug name, lowercased —
    /// e.g. `rust`, `ts`, `py`). A display hint, not load-bearing.
    pub lang: String,
    /// Count of DISTINCT top-level symbol names this file defines. This is the PRE-filter
    /// count: the raw number of named top-level items, BEFORE the [`MIN_SYMBOL_LEN`] /
    /// [`MAX_DEFINING_FILES`] edge filters (which affect edges, not this tally).
    pub defined_symbols: u32,
    /// In-degree: number of DISTINCT other files that reference a symbol defined in this
    /// file. The centrality measure the spine ranks on.
    pub in_degree: u32,
    /// Out-degree: number of DISTINCT other files this file references a symbol of.
    pub out_degree: u32,
}

/// One file on the architectural spine: a high-in-degree node plus the names that make
/// it central. Ordered by in-degree desc, then path asc (deterministic tie-break).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpineFile {
    /// Project-root-relative path (`/`-separated), matching the corresponding
    /// [`FileNode::path`].
    pub path: String,
    /// The file's in-degree (see [`FileNode::in_degree`]).
    pub in_degree: u32,
    /// Up to [`TOP_REFERENCED_SYMBOLS`] of this file's defined symbols that other files
    /// reference, ranked by distinct-referrer count (desc), then name (asc). The "why
    /// this file is central" hint for the planner.
    pub top_referenced_symbols: Vec<String>,
}

/// The deterministic cross-file structure graph for a project root. Serializable for the
/// planner's STRUCTURE phase + later MCP exposure (camelCase wire shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructureGraph {
    /// Every parsed source file, sorted by path asc (deterministic).
    pub files: Vec<FileNode>,
    /// The architectural spine: up to [`SPINE_MAX`] entries, the highest-in-degree files
    /// (zero-in-degree files are NEVER included). Fewer when the project is small; never
    /// padded with zero-in-degree files. Sorted in-degree desc, path asc.
    pub spine: Vec<SpineFile>,
    /// Number of source files actually parsed into the graph.
    pub scanned: u32,
    /// Number of files skipped because they exceeded [`MAX_FILE_BYTES`].
    pub skipped_too_large: u32,
    /// Number of files skipped because their extension maps to no parseable
    /// [`FileLang`] (or a grammar-less lang). NOT an error — just not graph material.
    pub skipped_unsupported: u32,
    /// Number of files skipped because they could not be stat'd, read, or carried a
    /// non-UTF-8 path segment (a node key must be lossless). NOT an error — just dropped
    /// from the graph, but COUNTED here so `scanned + skipped_* == files considered`.
    pub skipped_unreadable: u32,
    /// `true` when the walk hit a hard bound ([`MAX_FILES`] parsed files or
    /// [`MAX_WALK_ENTRIES`] total entries) and stopped early — the graph is PARTIAL and
    /// the planner must treat the spine as a best-effort sample, not the whole tree.
    pub capped: bool,
}

/// Per-file facts collected during the walk, before the cross-file edge pass. Kept in
/// walk order; the graph output is sorted at the end.
pub(crate) struct FileFacts {
    /// `/`-separated, root-relative path string (the stable node key).
    pub(crate) path: String,
    pub(crate) lang: FileLang,
    /// DISTINCT top-level defined symbol names (non-empty `ReviewItem.name`s).
    pub(crate) defined: BTreeSet<String>,
    /// Whole-file identifier set the grammar saw (referenced names).
    pub(crate) identifiers: BTreeSet<String>,
    /// Total source lines (for the CKG FILE node's end_line).
    pub(crate) total_lines: u32,
    /// The full parsed top-level items (kind/name/start_line/end_line) — the CKG's symbol nodes.
    /// Carried so the CKG reuses THIS parse instead of re-walking + re-parsing the tree.
    pub(crate) items: Vec<ReviewItem>,
}

/// Build the deterministic cross-file structure graph + spine for `project_root`.
///
/// Walks the tree (respecting `.gitignore` + hidden-file filtering via the `ignore`
/// crate, ALWAYS skipping [`SKIP_DIRS`]), parses each known-[`FileLang`] file ONCE via
/// [`extract::parse_file`], builds the name-resolution edge set (see the module header
/// for the heuristic + its limits), computes in-degree centrality, and ranks the spine.
///
/// Returns `Err` ONLY when the root itself cannot be walked (e.g. it does not exist or
/// is not a directory). Individual unreadable/oversized/unsupported files are SKIPPED
/// and counted, never fatal. The result is fully deterministic for a given tree.
pub fn build_structure_graph(project_root: &Path) -> Result<StructureGraph, String> {
    if !project_root.is_dir() {
        return Err(format!(
            "structure: project root is not a directory: {}",
            project_root.display()
        ));
    }

    let scan = collect_files(project_root);
    let facts = scan.facts;

    // ---- Build the name map: symbol_name -> set<file index>, with the generic-name
    // and short-name filters applied. A BTreeMap keyed on the name keeps construction
    // deterministic; we only ever iterate the per-file `identifiers` (also a BTreeSet),
    // so no HashMap order leaks anywhere. -----------------------------------------------
    let mut definers: HashMap<&str, BTreeSet<usize>> = HashMap::new();
    for (idx, f) in facts.iter().enumerate() {
        for name in &f.defined {
            if name.len() < MIN_SYMBOL_LEN {
                continue;
            }
            definers.entry(name.as_str()).or_default().insert(idx);
        }
    }
    // Drop ubiquitous names (defined in too many files) — they are noise, not signal.
    definers.retain(|_, files| files.len() <= MAX_DEFINING_FILES);

    // ---- Edge pass. `edges[a]` = set of files A references; we also accumulate, per
    // target file B and per symbol, the set of DISTINCT referrers (for in-degree and the
    // top-referenced-symbols hint). All keyed structures are deterministically ordered or
    // reduced to counts/sorted vecs before output. ------------------------------------
    let n = facts.len();
    let mut out_targets: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    let mut in_referrers: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    // Per target file: symbol_name -> distinct referrer file indices (for the hint).
    let mut symbol_referrers: Vec<BTreeMap<String, BTreeSet<usize>>> =
        vec![BTreeMap::new(); n];

    for (a, f) in facts.iter().enumerate() {
        // HTML files are NOT edge SOURCES: their "identifiers" are tag names + attribute
        // values (`div`, `src`, `id`, `login`, …), most of which are >= MIN_SYMBOL_LEN and
        // collide with code symbols, manufacturing phantom A->B edges. HTML rarely refers
        // to code symbols by name, so we suppress its out-edges entirely. An HTML file is
        // still a NODE and may still be an edge TARGET (its defined names stay in the name
        // map). See the module header's per-language noise note.
        if matches!(f.lang, FileLang::Html) {
            continue;
        }
        for ident in &f.identifiers {
            // `ident.len()` short-circuits before the map lookup; short names were never
            // inserted as definers anyway, but this avoids the hash entirely.
            if ident.len() < MIN_SYMBOL_LEN {
                continue;
            }
            let Some(defining_files) = definers.get(ident.as_str()) else {
                continue;
            };
            for &b in defining_files {
                if a == b {
                    continue; // a file referencing its own symbol is not a cross-file edge
                }
                out_targets[a].insert(b);
                in_referrers[b].insert(a);
                symbol_referrers[b]
                    .entry(ident.clone())
                    .or_default()
                    .insert(a);
            }
        }
    }

    // ---- Materialize the file nodes (sorted by path asc — facts are already in a
    // deterministic walk order, but we sort explicitly to make the contract independent
    // of the walker's order). ----------------------------------------------------------
    let mut files: Vec<FileNode> = facts
        .iter()
        .enumerate()
        .map(|(idx, f)| FileNode {
            path: f.path.clone(),
            lang: lang_name(f.lang).to_string(),
            defined_symbols: u32::try_from(f.defined.len()).unwrap_or(u32::MAX),
            in_degree: u32::try_from(in_referrers[idx].len()).unwrap_or(u32::MAX),
            out_degree: u32::try_from(out_targets[idx].len()).unwrap_or(u32::MAX),
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));

    // ---- Rank the spine: in-degree desc, then path asc (deterministic). Drop
    // zero-in-degree files (a leaf nothing references is never on the spine), then take
    // the top SPINE_MAX. ----------------------------------------------------------------
    let mut ranked: Vec<usize> = (0..n).filter(|&i| !in_referrers[i].is_empty()).collect();
    ranked.sort_by(|&x, &y| {
        in_referrers[y]
            .len()
            .cmp(&in_referrers[x].len())
            .then_with(|| facts[x].path.cmp(&facts[y].path))
    });
    ranked.truncate(SPINE_MAX);

    let spine: Vec<SpineFile> = ranked
        .into_iter()
        .map(|idx| SpineFile {
            path: facts[idx].path.clone(),
            in_degree: u32::try_from(in_referrers[idx].len()).unwrap_or(u32::MAX),
            top_referenced_symbols: top_referenced_symbols(&symbol_referrers[idx]),
        })
        .collect();

    Ok(StructureGraph {
        files,
        spine,
        scanned: u32::try_from(n).unwrap_or(u32::MAX),
        skipped_too_large: scan.skipped_too_large,
        skipped_unsupported: scan.skipped_unsupported,
        skipped_unreadable: scan.skipped_unreadable,
        capped: scan.capped,
    })
}

/// The argv token that selects the headless STRUCTURE bridge: `aspis-management
/// structure --root <path>`. Detected in `main` BEFORE the Tauri GUI builder runs, so the
/// process behaves as a one-shot CLI (no window) when invoked this way. Kept here next to
/// the builder so the bridge and the graph never drift.
pub const STRUCTURE_SUBCOMMAND: &str = "structure";

/// Build the structure graph for `root` and serialize it to a compact JSON string.
///
/// This is the pure core of the CLI bridge (no stdout, no process exit) so it is unit-
/// testable. Returns `Err` with a human-readable message when the root cannot be walked
/// or (vanishingly unlikely) the graph fails to serialize. The wire shape is exactly
/// [`StructureGraph`] (serde camelCase) — the Python `project_structure` tool parses this.
pub fn structure_cli_json(root: &Path) -> Result<String, String> {
    let graph = build_structure_graph(root)?;
    serde_json::to_string(&graph).map_err(|e| format!("structure: failed to serialize graph: {e}"))
}

/// Headless CLI bridge entry point. Given the FULL process args (`std::env::args()`),
/// returns:
///   - `None` when this is NOT a `structure` invocation (the caller proceeds to the GUI);
///   - `Some(0)` after printing the graph JSON to stdout on success;
///   - `Some(2)` after printing a one-line error to stderr on failure (bad/missing
///     `--root`, or an unwalkable root).
///
/// The caller (`main`) must `std::process::exit(code)` on `Some` so the GUI never starts.
/// We DETECT the subcommand from `args[1]` and require an explicit `--root <path>` so the
/// bridge can never accidentally fire for a normal app launch (which has no such argv).
pub fn run_structure_cli<I, S>(args: I) -> Option<i32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let argv: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
    // argv[0] is the program name; the subcommand is argv[1].
    if argv.get(1).map(String::as_str) != Some(STRUCTURE_SUBCOMMAND) {
        return None;
    }

    let root = match parse_root_flag(&argv[2..]) {
        Ok(root) => root,
        Err(msg) => {
            eprintln!("{msg}");
            return Some(2);
        }
    };

    match structure_cli_json(Path::new(&root)) {
        Ok(json) => {
            println!("{json}");
            Some(0)
        }
        Err(msg) => {
            eprintln!("{msg}");
            Some(2)
        }
    }
}

/// Parse exactly the `--root <path>` flag out of the trailing args (everything after the
/// `structure` subcommand). Requires the flag and a non-empty value; rejects unknown
/// tokens so a typo fails loudly instead of silently walking the wrong tree.
pub(crate) fn parse_root_flag(rest: &[String]) -> Result<String, String> {
    let mut root: Option<String> = None;
    let mut it = rest.iter();
    while let Some(tok) = it.next() {
        match tok.as_str() {
            "--root" => {
                let value = it
                    .next()
                    .ok_or_else(|| "structure: --root requires a path argument".to_string())?;
                if value.trim().is_empty() {
                    return Err("structure: --root path must not be empty".to_string());
                }
                root = Some(value.clone());
            }
            other => {
                return Err(format!("structure: unexpected argument '{other}'"));
            }
        }
    }
    root.ok_or_else(|| "structure: missing required --root <path>".to_string())
}

/// Rank a target file's referenced symbols by distinct-referrer count (desc), then name
/// (asc) for determinism, and return the top [`TOP_REFERENCED_SYMBOLS`] names.
fn top_referenced_symbols(symbol_referrers: &BTreeMap<String, BTreeSet<usize>>) -> Vec<String> {
    let mut ranked: Vec<(&String, usize)> = symbol_referrers
        .iter()
        .map(|(name, refs)| (name, refs.len()))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked
        .into_iter()
        .take(TOP_REFERENCED_SYMBOLS)
        .map(|(name, _)| name.clone())
        .collect()
}

/// The outcome of [`collect_files`]: the per-file facts plus the skip tallies and the
/// `capped` flag (set when a hard work bound stopped the walk early).
pub(crate) struct ScanResult {
    pub(crate) facts: Vec<FileFacts>,
    pub(crate) skipped_too_large: u32,
    pub(crate) skipped_unsupported: u32,
    pub(crate) skipped_unreadable: u32,
    pub(crate) capped: bool,
}

/// Walk `project_root` and collect the per-file facts for every parseable source file.
///
/// Uses a SINGLE-THREADED [`ignore::WalkBuilder`] for deterministic traversal order
/// (`sort_by_file_name`), with the standard filters on (hidden files + `.gitignore`/
/// `.ignore`/global gitignore) and an explicit [`SKIP_DIRS`] guard.
///
/// TWO independent hard bounds keep total work finite (either trips `capped`):
///   - at most [`MAX_FILES`] files are PARSED into the graph;
///   - at most [`MAX_WALK_ENTRIES`] entries are CONSIDERED in total (parsed + skipped of
///     every kind), so a tree with a huge number of skipped-but-stat'd files (which never
///     consume the parse cap) still terminates.
///
/// Per-file disposition: oversized files ([`MAX_FILE_BYTES`]) are counted in
/// `skipped_too_large`; files whose extension maps to no parseable [`FileLang`] in
/// `skipped_unsupported`; files that cannot be stat'd, cannot be read, or carry a
/// non-UTF-8 path segment (a node key MUST be lossless) in `skipped_unreadable`. None of
/// these is fatal — they are dropped from the graph but always counted, so every walk
/// entry that reached the per-file stage is accounted for.
pub(crate) fn collect_files(project_root: &Path) -> ScanResult {
    collect_files_bounded(project_root, MAX_FILES, MAX_WALK_ENTRIES)
}

/// The body of [`collect_files`] with the two work bounds injected, so the `capped`
/// branches are unit-testable with tiny inputs instead of materializing 2_000+ files. The
/// public path always uses [`MAX_FILES`] / [`MAX_WALK_ENTRIES`].
fn collect_files_bounded(
    project_root: &Path,
    max_files: usize,
    max_walk_entries: usize,
) -> ScanResult {
    use ignore::WalkBuilder;

    let mut facts: Vec<FileFacts> = Vec::new();
    let mut skipped_too_large: u32 = 0;
    let mut skipped_unsupported: u32 = 0;
    let mut skipped_unreadable: u32 = 0;
    // Total entries that reached the per-file disposition stage (parsed + every skip
    // kind). Bounds the walk independently of the parse cap.
    let mut visited: usize = 0;
    let mut capped = false;

    let walker = WalkBuilder::new(project_root)
        .standard_filters(true) // hidden files + .gitignore/.ignore/global-gitignore
        .parents(true)
        .require_git(false) // honor .gitignore even outside a git repo
        .sort_by_file_name(|a, b| a.cmp(b)) // deterministic traversal order
        .filter_entry(|entry| {
            // Skip our hard-blocked directories regardless of `.gitignore` state. We only
            // apply the name filter to directories so a FILE that happens to be named
            // e.g. `build` is still considered.
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            if is_dir {
                if let Some(name) = entry.file_name().to_str() {
                    if SKIP_DIRS.contains(&name) {
                        return false;
                    }
                }
            }
            true
        })
        .build();

    for result in walker {
        // Hard upper bound on the parse budget: stop once we have max_files parsed files.
        if facts.len() >= max_files {
            capped = true;
            break;
        }
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue, // an unreadable dir entry is skipped, not fatal
        };
        // Only files (the root and directories yield entries too). Directory entries do
        // NOT count toward the work bound — only candidate files do.
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }

        // Hard upper bound on TOTAL work: a tree with a huge count of skipped-but-stat'd
        // files would otherwise never consume the parse cap. Check BEFORE dispositioning
        // this entry so `visited` is the count we actually processed.
        if visited >= max_walk_entries {
            capped = true;
            break;
        }
        visited += 1;

        let path = entry.path();

        let lang = FileLang::from_path(path);
        if !is_parseable(lang) {
            skipped_unsupported += 1;
            continue;
        }

        // Size cap BEFORE reading the contents (cheap metadata probe).
        match entry.metadata() {
            Ok(md) if md.len() > MAX_FILE_BYTES => {
                skipped_too_large += 1;
                continue;
            }
            Ok(_) => {}
            Err(_) => {
                // Can't stat ⇒ skip, not fatal, but COUNT it (otherwise the file vanishes
                // from every tally and `scanned + skipped_* < visited`).
                skipped_unreadable += 1;
                continue;
            }
        }

        // A node key must be LOSSLESS: a non-UTF-8 path segment cannot be represented
        // without replacement chars (which would collide distinct files), so drop the
        // file before reading it and count it as unreadable.
        let Some(rel) = relative_path_string(project_root, path) else {
            skipped_unreadable += 1;
            continue;
        };

        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => {
                // Unreadable / non-UTF-8 contents ⇒ skip, not fatal, but COUNT it.
                skipped_unreadable += 1;
                continue;
            }
        };

        let parsed = extract::parse_file(&source, lang);
        let defined = defined_symbol_names(&parsed.items);
        let total_lines = parsed.total_lines;
        // Move the grammar's identifier set into a BTreeSet for deterministic iteration.
        let identifiers: BTreeSet<String> = parsed.identifiers.into_iter().collect();

        facts.push(FileFacts {
            path: rel,
            lang,
            defined,
            identifiers,
            total_lines,
            items: parsed.items,
        });
    }

    ScanResult {
        facts,
        skipped_too_large,
        skipped_unsupported,
        skipped_unreadable,
        capped,
    }
}

/// Is this language one our tree-sitter substrate actually parses into items +
/// identifiers? The grammar-less langs ([`FileLang::Shell`]/`Yaml`/`Sql`/`Dockerfile`/
/// `GithubActions`/`Css`) and [`FileLang::Other`] yield empty items + empty identifiers
/// (see `extract::parse_file`), so they contribute NOTHING to the graph — we skip them
/// up front and count them as unsupported rather than parsing to an empty result.
pub(crate) fn is_parseable(lang: FileLang) -> bool {
    matches!(
        lang,
        FileLang::Rust
            | FileLang::Ts
            | FileLang::Py
            | FileLang::Go
            | FileLang::Cpp
            | FileLang::Html
            | FileLang::Kotlin
    )
}

/// The DISTINCT, non-empty top-level defined symbol names from a file's review items.
fn defined_symbol_names(items: &[ReviewItem]) -> BTreeSet<String> {
    items
        .iter()
        .filter_map(|it| it.name.as_deref())
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .collect()
}

/// A stable, `/`-separated, root-relative path string for a file under `root`, or `None`
/// if any path segment is not valid UTF-8.
///
/// We join ONLY the `Normal` segments with `/`, pushing the separator solely when output
/// already has content — so non-`Normal` components that `components()` yields and we
/// skip (a leading `CurDir` `./`, a `RootDir`, a Windows `Prefix`) can NEVER manufacture a
/// spurious leading `/`. This matters because the root may be RELATIVE (e.g. `.`), in
/// which case a walked path is `./src/lib.rs` ⇒ components `[CurDir, Normal, Normal]`; a
/// raw enumerate index would push `/` before the first `Normal` and corrupt the key.
///
/// A node key MUST be lossless: a non-UTF-8 segment is reported as `None` (the caller
/// drops + counts the file) rather than lowered through `to_string_lossy`, whose U+FFFD
/// replacement would let two distinct non-UTF-8 names collide onto the same key. On
/// macOS/Linux/Windows, ordinary UTF-8 names are unaffected. Normalizing the separator
/// makes the key — and the whole graph + spine ordering — identical across OSes.
pub(crate) fn relative_path_string(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut out = String::new();
    for comp in rel.components() {
        if let std::path::Component::Normal(seg) = comp {
            let seg = seg.to_str()?; // non-UTF-8 ⇒ lossless key impossible ⇒ drop the file
            if !out.is_empty() {
                out.push('/');
            }
            out.push_str(seg);
        }
    }
    if out.is_empty() {
        // Degenerate: no Normal component (e.g. `path == root`). A real file under the
        // root always has at least one Normal segment, so this is unreachable in practice;
        // fall back to a lossless rendering and never emit an empty key.
        path.file_name()?.to_str().map(|s| s.to_string())
    } else {
        Some(out)
    }
}

/// Lowercase debug name of a [`FileLang`] for the [`FileNode::lang`] display field.
fn lang_name(lang: FileLang) -> &'static str {
    match lang {
        FileLang::Rust => "rust",
        FileLang::Ts => "ts",
        FileLang::Py => "py",
        FileLang::Go => "go",
        FileLang::Cpp => "cpp",
        FileLang::Html => "html",
        FileLang::Kotlin => "kotlin",
        FileLang::Shell => "shell",
        FileLang::Yaml => "yaml",
        FileLang::Sql => "sql",
        FileLang::Dockerfile => "dockerfile",
        FileLang::GithubActions => "githubActions",
        FileLang::Css => "css",
        FileLang::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Per-test temp root, matching the convention in `censor/detect.rs` (no `tempfile`
    /// dev-dep is declared in this crate, so we hand-roll a unique dir like the existing
    /// tests do). Cleaned up by the caller via `fs::remove_dir_all`.
    fn unique_temp_root(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "aspis-structure-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn node<'a>(graph: &'a StructureGraph, path: &str) -> &'a FileNode {
        graph
            .files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no file node for {path}; have: {:?}", graph.files))
    }

    #[test]
    fn empty_dir_yields_empty_graph_without_error() {
        let dir = unique_temp_root("empty");
        let graph = build_structure_graph(&dir).expect("empty dir must not error");
        assert!(graph.files.is_empty());
        assert!(graph.spine.is_empty());
        assert_eq!(graph.scanned, 0);
        assert_eq!(graph.skipped_too_large, 0);
        assert_eq!(graph.skipped_unsupported, 0);
        assert_eq!(graph.skipped_unreadable, 0);
        assert!(!graph.capped, "an empty dir never trips a work bound");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nonexistent_root_errors() {
        let dir = unique_temp_root("missing");
        let missing = dir.join("does-not-exist");
        assert!(build_structure_graph(&missing).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn central_rust_file_is_ranked_first_with_right_in_degree() {
        // `core.rs` defines `CoreThing`; three other files reference it. `core.rs` must be
        // spine #1 with in-degree 3, and the leaf has in-degree 0 and is NOT on the spine.
        let dir = unique_temp_root("central");
        write(
            &dir,
            "core.rs",
            "pub struct CoreThing { pub value: u32 }\n\
             pub fn make_core() -> CoreThing { CoreThing { value: 0 } }\n",
        );
        write(
            &dir,
            "a.rs",
            "fn use_a() { let _c: CoreThing = CoreThing { value: 1 }; }\n",
        );
        write(
            &dir,
            "b.rs",
            "fn use_b() -> CoreThing { CoreThing { value: 2 } }\n",
        );
        write(
            &dir,
            "c.rs",
            "fn use_c(x: CoreThing) -> u32 { x.value }\n",
        );
        // A leaf: defines its own thing, references nothing cross-file.
        write(&dir, "leaf.rs", "pub fn standalone_leaf() -> u32 { 42 }\n");

        let graph = build_structure_graph(&dir).expect("graph builds");

        // core.rs is the single highest-in-degree node: referenced by a.rs, b.rs, c.rs.
        let core = node(&graph, "core.rs");
        assert_eq!(core.in_degree, 3, "core.rs referenced by exactly 3 files");

        assert!(!graph.spine.is_empty(), "spine must not be empty");
        assert_eq!(graph.spine[0].path, "core.rs", "core.rs ranks #1");
        assert_eq!(graph.spine[0].in_degree, 3);
        assert!(
            graph.spine[0]
                .top_referenced_symbols
                .contains(&"CoreThing".to_string()),
            "CoreThing must surface as a top referenced symbol, got {:?}",
            graph.spine[0].top_referenced_symbols
        );

        // The leaf has in-degree 0 and is NOT on the spine.
        let leaf = node(&graph, "leaf.rs");
        assert_eq!(leaf.in_degree, 0, "leaf is referenced by nobody");
        assert!(
            graph.spine.iter().all(|s| s.path != "leaf.rs"),
            "a zero-in-degree leaf is never on the spine"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ubiquitous_name_is_dropped_by_generic_filter() {
        // `Helper` is defined in MAX_DEFINING_FILES + 1 files ⇒ dropped from the name map
        // ⇒ creates NO edges. A referrer of ONLY `Helper` therefore has out-degree 0 and
        // none of the `Helper` files gains in-degree from it.
        let dir = unique_temp_root("generic");
        let n_defs = MAX_DEFINING_FILES + 1;
        for i in 0..n_defs {
            write(
                &dir,
                &format!("def_{i}.rs"),
                "pub struct Helper { pub n: u32 }\n",
            );
        }
        // A referrer that ONLY mentions the ubiquitous `Helper` — no other cross-file name.
        write(
            &dir,
            "refer.rs",
            "fn use_helper() { let _h = Helper { n: 1 }; }\n",
        );

        let graph = build_structure_graph(&dir).expect("graph builds");

        let refer = node(&graph, "refer.rs");
        assert_eq!(
            refer.out_degree, 0,
            "edges to the ubiquitous `Helper` must be dropped"
        );
        // None of the Helper-defining files gained in-degree from `refer.rs`.
        for i in 0..n_defs {
            let def = node(&graph, &format!("def_{i}.rs"));
            assert_eq!(
                def.in_degree, 0,
                "def_{i}.rs must gain no in-degree from a dropped generic name"
            );
        }
        // With no edges at all, the spine is empty (no positive-in-degree file).
        assert!(graph.spine.is_empty(), "no edges ⇒ empty spine");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn short_name_is_ignored_by_min_len_filter() {
        // A defined symbol shorter than MIN_SYMBOL_LEN (here `Io`, len 2) never creates an
        // edge even though another file references the identifier `Io`.
        assert!(MIN_SYMBOL_LEN >= 3, "test assumes a >=3 threshold");
        let dir = unique_temp_root("shortname");
        write(&dir, "def.rs", "pub struct Io { pub fd: u32 }\n");
        write(&dir, "ref.rs", "fn use_io() { let _x = Io { fd: 0 }; }\n");

        let graph = build_structure_graph(&dir).expect("graph builds");
        let def = node(&graph, "def.rs");
        assert_eq!(
            def.in_degree, 0,
            "a sub-threshold short name must create no edge"
        );
        assert!(graph.spine.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsupported_and_oversized_files_are_skipped_and_counted() {
        let dir = unique_temp_root("skip");
        // One supported, small file (parsed).
        write(&dir, "ok.rs", "pub fn a_real_function() {}\n");
        // Unsupported extension (no grammar / Other) — counted as unsupported.
        write(&dir, "notes.txt", "just some prose, not code\n");
        write(&dir, "data.json", "{ \"k\": 1 }\n");
        // Oversized supported file — counted as too-large, NOT parsed.
        let big = format!(
            "pub fn big() {{}}\n{}",
            "// padding line to exceed the size cap\n".repeat(20_000)
        );
        assert!(
            big.len() as u64 > MAX_FILE_BYTES,
            "fixture must exceed the size cap"
        );
        write(&dir, "huge.rs", &big);

        let graph = build_structure_graph(&dir).expect("graph builds");

        assert_eq!(graph.scanned, 1, "only ok.rs is a parsed source file");
        assert!(
            graph.files.iter().any(|f| f.path == "ok.rs"),
            "ok.rs must be in the graph"
        );
        assert!(
            graph.files.iter().all(|f| f.path != "huge.rs"),
            "the oversized file must NOT be a graph node"
        );
        assert_eq!(graph.skipped_too_large, 1, "huge.rs counted once");
        assert_eq!(
            graph.skipped_unsupported, 2,
            "notes.txt + data.json counted as unsupported"
        );
        assert_eq!(
            graph.skipped_unreadable, 0,
            "all fixtures are readable + valid UTF-8 paths"
        );
        assert!(!graph.capped, "4 entries is far below either work bound");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_artifact_dirs_are_skipped() {
        // A file inside `node_modules`/`target` must never be scanned, even though it is a
        // supported language.
        let dir = unique_temp_root("artifacts");
        write(&dir, "src_main.rs", "pub fn entry() {}\n");
        write(
            &dir,
            "node_modules/dep/index.ts",
            "export function vendored() {}\n",
        );
        write(&dir, "target/debug/built.rs", "pub fn artifact() {}\n");

        let graph = build_structure_graph(&dir).expect("graph builds");
        assert_eq!(graph.scanned, 1, "only the real source file is scanned");
        assert!(graph.files.iter().any(|f| f.path == "src_main.rs"));
        assert!(
            graph.files.iter().all(|f| !f.path.contains("node_modules")),
            "node_modules must be skipped"
        );
        assert!(
            graph.files.iter().all(|f| !f.path.contains("target")),
            "target must be skipped"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cross_language_ts_central_file() {
        // A TS fixture: `model.ts` defines `Widget`, referenced by two views.
        let dir = unique_temp_root("ts");
        write(
            &dir,
            "model.ts",
            "export class Widget { id = 0 }\nexport function makeWidget(): Widget { return new Widget(); }\n",
        );
        write(
            &dir,
            "view_a.ts",
            "function renderA(w: Widget) { return w.id; }\n",
        );
        write(
            &dir,
            "view_b.ts",
            "function renderB(): Widget { return new Widget(); }\n",
        );

        let graph = build_structure_graph(&dir).expect("graph builds");
        let model = node(&graph, "model.ts");
        assert_eq!(model.in_degree, 2, "model.ts referenced by both views");
        assert_eq!(graph.spine[0].path, "model.ts");
        assert!(graph.spine[0]
            .top_referenced_symbols
            .contains(&"Widget".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_is_deterministic_across_runs() {
        // Same tree ⇒ byte-identical serialized graph on a second run.
        let dir = unique_temp_root("determinism");
        write(&dir, "core.rs", "pub struct CoreThing {}\npub fn helper_core() {}\n");
        write(&dir, "a.rs", "fn ua() -> CoreThing { CoreThing {} }\n");
        write(&dir, "b.rs", "fn ub() -> CoreThing { CoreThing {} }\n");
        write(&dir, "mid.rs", "pub struct MidThing {}\nfn um() -> CoreThing { CoreThing {} }\n");
        write(&dir, "z.rs", "fn uz(m: MidThing) {}\n");

        let g1 = build_structure_graph(&dir).expect("run 1");
        let g2 = build_structure_graph(&dir).expect("run 2");
        let j1 = serde_json::to_string(&g1).expect("serialize 1");
        let j2 = serde_json::to_string(&g2).expect("serialize 2");
        assert_eq!(j1, j2, "same tree must serialize identically");
        // And the structs compare equal.
        assert_eq!(g1, g2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spine_is_capped_at_spine_max() {
        // Build more than SPINE_MAX referenced files; the spine must cap at SPINE_MAX and
        // be ordered by in-degree desc then path asc. Each `hubN.rs` defines a uniquely
        // named symbol referenced by a distinct number of referrers so the ordering is
        // unambiguous.
        let dir = unique_temp_root("cap");
        let hub_count = SPINE_MAX + 3;
        for h in 0..hub_count {
            // hub h defines `HubSymbol{h}` and is referenced by (h + 1) referrers.
            write(
                &dir,
                &format!("hub{h}.rs"),
                &format!("pub struct HubSymbol{h} {{}}\n"),
            );
            for r in 0..=h {
                write(
                    &dir,
                    &format!("ref_{h}_{r}.rs"),
                    &format!("fn use_it() -> HubSymbol{h} {{ HubSymbol{h} {{}} }}\n"),
                );
            }
        }

        let graph = build_structure_graph(&dir).expect("graph builds");
        assert_eq!(graph.spine.len(), SPINE_MAX, "spine capped at SPINE_MAX");
        // The top spine file is the most-referenced hub (hub_count-1, with hub_count refs).
        assert_eq!(
            graph.spine[0].path,
            format!("hub{}.rs", hub_count - 1),
            "highest-in-degree hub ranks #1"
        );
        // In-degree must be non-increasing down the spine (the ranking contract).
        for w in graph.spine.windows(2) {
            assert!(
                w[0].in_degree >= w[1].in_degree,
                "spine must be sorted by in-degree desc"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// Process-wide guard: `set_current_dir` is process-global, and cargo runs unit tests
    /// multi-threaded. Any test that mutates the cwd must hold this so it never races a
    /// concurrent build (which resolves a relative root against the cwd).
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn relative_path_string_has_no_spurious_leading_slash() {
        // Unit-level proof of the fix: when `strip_prefix` FAILS, the fallback walks the
        // raw path whose components begin with `CurDir` (`./`). The separator must be
        // pushed only when output is already non-empty — NOT keyed on the raw component
        // index (which counts the skipped `CurDir`) — so no leading `/` is manufactured.
        let root = Path::new("nonmatching_root");
        let walked = Path::new("./proj/src/lib.rs"); // strip_prefix(root) fails ⇒ fallback
        let key = relative_path_string(root, walked).expect("utf-8 segments");
        assert_eq!(
            key, "proj/src/lib.rs",
            "leading CurDir must not produce a leading '/'"
        );
        assert!(!key.starts_with('/'), "node key must never start with '/'");
    }

    #[test]
    fn relative_root_scan_matches_absolute_root_without_leading_slash() {
        // Build the SAME tree two ways: via its absolute root, and via a RELATIVE `./name`
        // root (cwd set to the temp parent). Both must yield identical node keys, and the
        // relative-root keys must carry NO leading `/` (the BLOCKER this fix targets).
        let _guard = CWD_LOCK.lock().expect("cwd lock");

        let parent = unique_temp_root("relroot");
        let proj = parent.join("proj");
        fs::create_dir_all(&proj).unwrap();
        write(&proj, "core.rs", "pub struct CoreThing {}\n");
        write(&proj, "src/a.rs", "fn ua() -> CoreThing { CoreThing {} }\n");
        write(&proj, "src/b.rs", "fn ub() -> CoreThing { CoreThing {} }\n");

        // Absolute-root build (the reference).
        let abs = build_structure_graph(&proj).expect("absolute-root build");

        // Relative-root build: cwd := parent, root := "./proj".
        let prev = std::env::current_dir().expect("save cwd");
        std::env::set_current_dir(&parent).expect("enter temp parent");
        let rel = build_structure_graph(Path::new("./proj"));
        std::env::set_current_dir(&prev).expect("restore cwd");
        let rel = rel.expect("relative-root build");

        // No key may start with '/' (the bug emitted `/src/a.rs`).
        for f in &rel.files {
            assert!(
                !f.path.starts_with('/'),
                "relative-root key must not start with '/': {:?}",
                f.path
            );
        }
        // Concrete expected keys (forward-slash, root-relative).
        assert!(
            rel.files.iter().any(|f| f.path == "src/a.rs"),
            "expected key src/a.rs, got {:?}",
            rel.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        // The two builds must be byte-identical (path keys drive every downstream sort).
        let ja = serde_json::to_string(&abs).expect("ser abs");
        let jr = serde_json::to_string(&rel).expect("ser rel");
        assert_eq!(
            ja, jr,
            "relative-root and absolute-root scans of the same tree must be identical"
        );

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn walk_is_bounded_by_total_entries_not_just_parsed() {
        // The walk must stop on TOTAL entries considered, so a tree of unsupported files
        // (which never consume the parse cap) cannot walk unboundedly. We cannot afford to
        // materialize MAX_WALK_ENTRIES files in a unit test, so we assert the contract
        // through `collect_files` directly with a tiny tree and verify the bookkeeping
        // identity instead, plus prove the parse cap sets `capped`.
        let dir = unique_temp_root("walkbound");
        // A mix: 2 parsed, 3 unsupported, all readable. `visited` == 5; none capped.
        write(&dir, "a.rs", "pub fn a_function() {}\n");
        write(&dir, "b.rs", "pub fn b_function() {}\n");
        write(&dir, "x1.txt", "prose\n");
        write(&dir, "x2.txt", "prose\n");
        write(&dir, "x3.json", "{}\n");

        let scan = collect_files(&dir);
        // Bookkeeping identity: every file-stage entry is in exactly one bucket.
        let accounted = scan.facts.len()
            + scan.skipped_too_large as usize
            + scan.skipped_unsupported as usize
            + scan.skipped_unreadable as usize;
        assert_eq!(accounted, 5, "every walk entry must land in exactly one tally");
        assert_eq!(scan.facts.len(), 2, "two parsed .rs files");
        assert_eq!(scan.skipped_unsupported, 3, "three unsupported files");
        assert!(!scan.capped, "5 entries is below both work bounds");

        // The constants encode a hard upper bound on total work (both caps finite, and the
        // walk-entry cap is the absolute ceiling). This is the invariant the loop relies on.
        assert!(MAX_WALK_ENTRIES >= MAX_FILES, "walk cap must not undercut parse cap");
        assert!(MAX_WALK_ENTRIES > 0 && MAX_FILES > 0, "both bounds must be positive");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_cap_sets_capped_flag_and_truncates() {
        // Drive the PARSE cap with an injected tiny budget (max_files = 1): the walk must
        // stop after the first parsed file and set `capped`. Uses the bounded seam so we
        // never have to materialize MAX_FILES (2_000) files in a unit test.
        let dir = unique_temp_root("parsecap");
        write(&dir, "a.rs", "pub fn a_fn() {}\n");
        write(&dir, "b.rs", "pub fn b_fn() {}\n");
        write(&dir, "c.rs", "pub fn c_fn() {}\n");

        let scan = collect_files_bounded(&dir, 1, usize::MAX);
        assert_eq!(scan.facts.len(), 1, "parse cap of 1 stops after one file");
        assert!(scan.capped, "hitting the parse cap must set capped");

        // And the FALSE case: a tiny project under the real bounds is never capped.
        let g = build_structure_graph(&dir).expect("builds");
        assert!(!g.capped, "3 files is far below the real bounds");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn walk_entry_cap_sets_capped_flag() {
        // Drive the TOTAL-work cap with an injected tiny budget (max_walk_entries = 2):
        // even unsupported files (which never consume the parse cap) count toward it, so a
        // tree of mostly-skipped files still terminates and trips `capped`.
        let dir = unique_temp_root("walkcap");
        // 1 parsed + several unsupported; the walk-entry cap of 2 must stop early.
        write(&dir, "a.rs", "pub fn a_fn() {}\n");
        write(&dir, "x1.txt", "prose\n");
        write(&dir, "x2.txt", "prose\n");
        write(&dir, "x3.txt", "prose\n");
        write(&dir, "x4.txt", "prose\n");

        let scan = collect_files_bounded(&dir, usize::MAX, 2);
        let considered = scan.facts.len()
            + scan.skipped_too_large as usize
            + scan.skipped_unsupported as usize
            + scan.skipped_unreadable as usize;
        assert_eq!(considered, 2, "walk-entry cap of 2 stops after two entries");
        assert!(scan.capped, "hitting the walk-entry cap must set capped");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_file_is_counted_not_silently_dropped() {
        // A parseable file that cannot be read must be COUNTED in skipped_unreadable, so
        // the bookkeeping identity holds (no file vanishes from every tally). On unix we
        // create a supported-extension file with no read permission; the size probe (stat)
        // still succeeds, but `read_to_string` fails ⇒ skipped_unreadable += 1.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = unique_temp_root("unreadable");
            write(&dir, "ok.rs", "pub fn a_real_function() {}\n");
            let secret = dir.join("secret.rs");
            fs::write(&secret, "pub fn hidden() {}\n").unwrap();
            // Remove all permissions: stat succeeds, open fails.
            fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();

            let scan = collect_files(&dir);
            // Restore perms so the dir can be cleaned up.
            let _ = fs::set_permissions(&secret, fs::Permissions::from_mode(0o644));

            assert_eq!(scan.facts.len(), 1, "only ok.rs parses");
            assert_eq!(
                scan.skipped_unreadable, 1,
                "the unreadable file must be counted, not silently dropped"
            );
            // Bookkeeping identity holds: 2 file-stage entries, 1 parsed + 1 unreadable.
            let accounted = scan.facts.len()
                + scan.skipped_too_large as usize
                + scan.skipped_unsupported as usize
                + scan.skipped_unreadable as usize;
            assert_eq!(accounted, 2, "every entry accounted for");

            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn html_file_is_a_node_but_never_an_edge_source() {
        // HTML identifiers are tag/attribute noise. An HTML file that "references" a code
        // symbol name (here `Widget`) must NOT create an A->B edge: the code file's
        // in-degree stays 0 from the HTML file, and the HTML file's out-degree is 0. The
        // HTML file is still present as a NODE.
        let dir = unique_temp_root("htmledge");
        // A real code definition of `Widget`.
        write(
            &dir,
            "model.ts",
            "export class Widget { id = 0 }\nexport function makeWidget(): Widget { return new Widget(); }\n",
        );
        // An HTML page whose tag/class/id text includes `Widget` (and other code-like
        // tokens). These must NOT manufacture edges into model.ts.
        write(
            &dir,
            "page.html",
            "<!doctype html><html><body>\
             <div id=\"Widget\" class=\"Widget makeWidget\">Widget</div>\
             <button data-widget=\"Widget\">go</button>\
             </body></html>\n",
        );

        let graph = build_structure_graph(&dir).expect("graph builds");

        // The HTML file IS a node.
        assert!(
            graph.files.iter().any(|f| f.path == "page.html"),
            "page.html must still be a graph node"
        );
        // The HTML file has out-degree 0 (suppressed as an edge source).
        let page = node(&graph, "page.html");
        assert_eq!(page.out_degree, 0, "HTML file must never be an edge source");
        // model.ts gains NO in-degree from the HTML file (the only other file here).
        let model = node(&graph, "model.ts");
        assert_eq!(
            model.in_degree, 0,
            "an HTML file must not raise a code file's in-degree"
        );
        // With no edges, the spine is empty.
        assert!(graph.spine.is_empty(), "HTML-only references ⇒ no spine");

        let _ = fs::remove_dir_all(&dir);
    }

    // ---- CLI bridge (`aspis-management structure --root <path>`) ----------------------

    #[test]
    fn cli_non_structure_argv_returns_none() {
        // A normal app launch (no `structure` subcommand) must NOT trigger the bridge,
        // so `main` proceeds to the GUI. Both an empty argv tail and an unrelated first
        // arg yield None.
        assert_eq!(run_structure_cli(["aspis-management"]), None);
        assert_eq!(
            run_structure_cli(["aspis-management", "--some-tauri-flag"]),
            None
        );
    }

    #[test]
    fn cli_emits_graph_json_and_exits_zero_for_a_valid_root() {
        // argv -> JSON to stdout, exit 0. We assert the pure core (`structure_cli_json`,
        // which `run_structure_cli` prints verbatim) so the test captures the exact wire
        // bytes without intercepting stdout, AND that the dispatcher returns Some(0).
        let dir = unique_temp_root("cli-ok");
        write(
            &dir,
            "core.rs",
            "pub struct CoreThing { pub value: u32 }\n\
             pub fn make_core() -> CoreThing { CoreThing { value: 0 } }\n",
        );
        write(&dir, "a.rs", "fn ua() -> CoreThing { CoreThing { value: 1 } }\n");

        let json = structure_cli_json(&dir).expect("cli json builds");
        // It is the StructureGraph wire shape (camelCase) and round-trips.
        let parsed: StructureGraph = serde_json::from_str(&json).expect("valid graph json");
        assert!(
            parsed.files.iter().any(|f| f.path == "core.rs"),
            "core.rs must be a node in the CLI output"
        );
        assert!(
            json.contains("\"spine\"") && json.contains("\"scanned\""),
            "wire shape must carry spine + summary counts"
        );

        // The dispatcher recognizes the subcommand + --root and reports success.
        let root_s = dir.to_string_lossy().into_owned();
        let code = run_structure_cli([
            "aspis-management".to_string(),
            STRUCTURE_SUBCOMMAND.to_string(),
            "--root".to_string(),
            root_s,
        ]);
        assert_eq!(code, Some(0), "valid root must exit 0");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cli_bad_root_exits_nonzero() {
        // A `structure` invocation whose root does not exist must return a NON-ZERO code
        // (so the spawning Python tool sees the failure), not None and not 0.
        let dir = unique_temp_root("cli-bad");
        let missing = dir.join("does-not-exist");
        let code = run_structure_cli([
            "aspis-management".to_string(),
            STRUCTURE_SUBCOMMAND.to_string(),
            "--root".to_string(),
            missing.to_string_lossy().into_owned(),
        ]);
        assert_eq!(code, Some(2), "an unwalkable root must exit non-zero");
        // And the pure core surfaces an Err (never a panic).
        assert!(structure_cli_json(&missing).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cli_missing_or_malformed_root_flag_exits_nonzero() {
        // The subcommand is present but `--root` is absent / value-less / has an unknown
        // extra token: each is a usage error → Some(2), never None (which would fall
        // through to the GUI) and never 0.
        assert_eq!(
            run_structure_cli(["aspis-management", STRUCTURE_SUBCOMMAND]),
            Some(2),
            "missing --root is a usage error"
        );
        assert_eq!(
            run_structure_cli(["aspis-management", STRUCTURE_SUBCOMMAND, "--root"]),
            Some(2),
            "--root with no value is a usage error"
        );
        assert_eq!(
            run_structure_cli([
                "aspis-management",
                STRUCTURE_SUBCOMMAND,
                "--bogus",
                "x",
            ]),
            Some(2),
            "an unknown flag is a usage error"
        );
    }
}
