//! Code Knowledge Graph (CKG) builder + headless `ckg --root <path>` CLI bridge.
//!
//! REUSES the Censor's tree-sitter parse (`censor::extract::parse_file`) and the
//! `structure` module's walk helpers (SKIP_DIRS / is_parseable / MAX_FILE_BYTES /
//! relative_path_string / parse_root_flag) so the CKG and the structure graph share ONE
//! source of truth for what gets walked and parsed. This slice emits FILE + symbol
//! nodes, CONTAIN edges, and IMPORT edges (resolved via `graph::resolve_import_edges`
//! from the real tree-sitter import capture in `censor::extract`).
//! The wire shape mirrors `structure`'s bridge: the Python side shells `<app_bin> ckg --root`.

use std::path::Path;

use crate::backend::{graph, structure};

/// A node in the Code Knowledge Graph.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CkgNode {
    pub id: String,
    pub kind: String,
    pub name: Option<String>,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub lang: String,
}

/// An edge in the Code Knowledge Graph.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CkgEdge {
    pub src: String,
    pub dst: String,
    pub kind: String,
}

/// The root graph structure (serde camelCase — the Python CKG ingester parses this).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CkgGraph {
    pub nodes: Vec<CkgNode>,
    pub edges: Vec<CkgEdge>,
    /// True when the walk hit a hard bound (`MAX_FILES` parsed or `MAX_WALK_ENTRIES`
    /// considered) and stopped early — the graph is PARTIAL. Mirrors `StructureGraph.capped`.
    pub capped: bool,
}

/// The argv token that selects the headless CKG bridge: `devboule ckg --root <path>`.
pub const CKG_SUBCOMMAND: &str = "ckg";

/// Build the Code Knowledge Graph for `project_root`: one FILE node per parseable file plus
/// one symbol node per top-level item (function/struct/class/...), with a CONTAIN edge from
/// each file to its symbols. Reuses the SAME bounded, gitignore-aware, deterministic walk as
/// `structure::build_structure_graph`.
pub fn build_ckg_graph(project_root: &Path) -> Result<CkgGraph, String> {
    // Fail-CLOSED on a bad root (mirrors structure.rs) — Ok(empty) would let the ingester wipe the graph.
    if !project_root.is_dir() {
        return Err(format!(
            "ckg: project root is not a directory: {}",
            project_root.display()
        ));
    }

    // DE-DUPLICATION: ONE shared walk + ONE tree-sitter parse per file. Reuse structure's
    // `collect_files` (which now carries `items` + `total_lines` in FileFacts) instead of a SECOND
    // WalkBuilder pass that would re-parse every file. Same bounds/skip rules/`capped` as structure.
    let scan = structure::collect_files(project_root);

    let mut nodes: Vec<CkgNode> = Vec::new();
    let mut edges: Vec<CkgEdge> = Vec::new();

    for f in &scan.facts {
        // Skip empty files: a FILE node with start_line=1 > end_line=0 is an inverted range.
        if f.total_lines == 0 {
            continue;
        }
        let lang_str = format!("{:?}", f.lang);

        // One FILE node, keyed by the relative path (the file_id convention).
        nodes.push(CkgNode {
            id: f.path.clone(),
            kind: "FILE".to_string(),
            name: None,
            file: f.path.clone(),
            start_line: 1,
            end_line: f.total_lines,
            lang: lang_str.clone(),
        });

        // One symbol node per top-level item + a CONTAIN edge file -> symbol. The item index
        // disambiguates two items that share a start-end line span (e.g. `fn a(){} fn b(){}`).
        for (idx, item) in f.items.iter().enumerate() {
            let node_id = format!("{}#{}-{}-{}", f.path, item.start_line, item.end_line, idx);
            nodes.push(CkgNode {
                id: node_id.clone(),
                kind: item.kind.clone(),
                name: item.name.clone(),
                file: f.path.clone(),
                start_line: item.start_line,
                end_line: item.end_line,
                lang: lang_str.clone(),
            });
            edges.push(CkgEdge {
                src: f.path.clone(),
                dst: node_id,
                kind: "CONTAIN".to_string(),
            });
        }
    }

    // ---- IMPORT edges: resolve real import references from the parse substrate.
    let import_edges = graph::resolve_import_edges(&scan.facts);
    for e in &import_edges {
        edges.push(CkgEdge {
            src: e.from.clone(),
            dst: e.to.clone(),
            kind: "IMPORT".to_string(),
        });
    }

    // Deterministic order so the Python ingester can diff two dumps stably.
    nodes.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.start_line.cmp(&b.start_line))
            .then(a.id.cmp(&b.id))
    });
    edges.sort_by(|a, b| {
        a.src
            .cmp(&b.src)
            .then(a.dst.cmp(&b.dst))
            .then(a.kind.cmp(&b.kind))
    });

    Ok(CkgGraph {
        nodes,
        edges,
        capped: scan.capped,
    })
}

/// The pure core of the CLI bridge (no stdout/exit) — unit-testable. Mirrors
/// `structure::structure_cli_json`.
pub fn ckg_cli_json(root: &Path) -> Result<String, String> {
    let graph = build_ckg_graph(root)?;
    serde_json::to_string(&graph).map_err(|e| format!("ckg: failed to serialize graph: {e}"))
}

/// Headless CLI bridge entry point. `None` when not a `ckg` invocation; `Some(0)` after
/// printing the graph JSON; `Some(2)` on a bad `--root` or unwalkable root. Mirrors
/// `structure::run_structure_cli`.
pub fn run_ckg_cli<I, S>(args: I) -> Option<i32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let argv: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
    if argv.get(1).map(String::as_str) != Some(CKG_SUBCOMMAND) {
        return None;
    }
    let root = match structure::parse_root_flag(&argv[2..]) {
        Ok(r) => r,
        Err(msg) => {
            eprintln!("{msg}");
            return Some(2);
        }
    };
    match ckg_cli_json(Path::new(&root)) {
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

#[cfg(test)]
mod tests {
    fn unique_temp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ckg-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &std::path::Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn build_ckg_graph_emits_file_and_symbol_nodes_with_contain_edges() {
        let dir = unique_temp_root("basic");
        write(
            &dir,
            "src/lib.rs",
            "pub fn alpha() -> u32 { 1 }\npub struct Beta { pub x: u32 }\n",
        );
        let g = super::build_ckg_graph(&dir).unwrap();

        let file_node = g
            .nodes
            .iter()
            .find(|n| n.kind == "FILE" && n.file.ends_with("lib.rs"))
            .expect("Expected a FILE node for lib.rs");

        let func_node = g
            .nodes
            .iter()
            .find(|n| n.kind == "function_item" && n.name.as_deref() == Some("alpha"))
            .expect("Expected a function_item node named 'alpha'");

        let has_contain_edge = g
            .edges
            .iter()
            .any(|e| e.kind == "CONTAIN" && e.src == file_node.id && e.dst == func_node.id);
        assert!(
            has_contain_edge,
            "Expected a CONTAIN edge from {} to {}",
            file_node.id, func_node.id
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_edges_from_a_to_b() {
        let dir = unique_temp_root("import");
        // A imports symbols from B
        write(
            &dir,
            "src/a.rs",
            "use crate::b::Thing;\npub fn caller() -> Thing { Thing { x: 1 } }\n",
        );
        write(
            &dir,
            "src/b.rs",
            "pub struct Thing { pub x: u32 }\n",
        );
        let g = super::build_ckg_graph(&dir).unwrap();

        // Both files have FILE nodes
        let a_file = g.nodes.iter().find(|n| n.id == "src/a.rs").expect("FILE node for a.rs");
        let b_file = g.nodes.iter().find(|n| n.id == "src/b.rs").expect("FILE node for b.rs");
        assert_eq!(a_file.kind, "FILE");
        assert_eq!(b_file.kind, "FILE");

        // CONTAIN edges exist
        let contain_a = g.edges.iter().any(|e| e.kind == "CONTAIN" && e.src == "src/a.rs");
        assert!(contain_a, "CONTAIN edges must be present");

        // IMPORT edge from a.rs -> b.rs
        let import_edge = g.edges.iter().find(|e| {
            e.kind == "IMPORT" && e.src == "src/a.rs" && e.dst == "src/b.rs"
        });
        assert!(
            import_edge.is_some(),
            "Expected IMPORT edge src/a.rs -> src/b.rs, got edges: {:?}",
            g.edges.iter().filter(|e| e.kind == "IMPORT").collect::<Vec<_>>()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

}