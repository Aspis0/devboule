//! SQLite-backed code-knowledge-graph store (symbol nodes + typed edges).
//!
//! Port of `oracle/store/ckg_store.py::CkgStore`. Same WAL + busy-timeout
//! access pattern as `SqliteStore` so a single resident server can read while
//! a re-index writes. Populated from the Rust `ckg --root <path>` bridge.
//!
//! ## Schema contract
//!
//! The `ckg_nodes` / `ckg_edges` tables, their columns, and the five indexes
//! below are a HARD WIRE CONTRACT: `oracle/aspm/aspis_mcp.py` (the Python
//! MCP server) reads this DB directly via `oracle/store/ckg_store.py::CkgStore`.
//! Any change to column names, table names, or the PRIMARY KEYs must be mirrored
//! in both Rust and Python or the MCP will break.

use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, Connection, ToSql};
use std::path::{Path, PathBuf};

/// Busy timeout in milliseconds. Mirrors `ckg_store.py::_BUSY_TIMEOUT_MS`.
const BUSY_TIMEOUT_MS: i32 = 5000;

/// Exact DDL (every statement `IF NOT EXISTS`), byte-equivalent to
/// `ckg_store.py::_init_schema`.
const SCHEMA_DDL: &str = "
CREATE TABLE IF NOT EXISTS ckg_nodes (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT,
    file TEXT NOT NULL,
    start_line INTEGER,
    end_line INTEGER,
    lang TEXT
);
CREATE INDEX IF NOT EXISTS idx_ckg_nodes_file ON ckg_nodes(file);
CREATE INDEX IF NOT EXISTS idx_ckg_nodes_name ON ckg_nodes(name);

CREATE TABLE IF NOT EXISTS ckg_edges (
    src TEXT NOT NULL,
    dst TEXT NOT NULL,
    kind TEXT NOT NULL,
    src_file TEXT NOT NULL,
    PRIMARY KEY (src, dst, kind)
);
CREATE INDEX IF NOT EXISTS idx_ckg_edges_dst ON ckg_edges(dst, kind);
CREATE INDEX IF NOT EXISTS idx_ckg_edges_src ON ckg_edges(src, kind);
CREATE INDEX IF NOT EXISTS idx_ckg_edges_srcfile ON ckg_edges(src_file);
";

/// A node row in the `ckg_nodes` table.
///
/// Field names mirror the SQLite columns verbatim (snake_case, matching the
/// Python `ckg_store.py::CkgNode` dict keys).
#[derive(Debug, Clone)]
pub struct CkgNodeRow {
    pub id: String,
    pub kind: String,
    pub name: Option<String>,
    pub file: String,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub lang: Option<String>,
}

/// An edge row in the `ckg_edges` table.
///
/// `src_file` is the file that the `src` node lives in (attached by the
/// ingester; see `oracle/ingestion/ckg_index.py::_attach_src_file`).
#[derive(Debug, Clone)]
pub struct CkgEdgeRow {
    pub src: String,
    pub dst: String,
    pub kind: String,
    pub src_file: String,
}

/// SQLite code-knowledge-graph store. Mirrors `oracle/store/ckg_store.py::CkgStore`.
pub struct CkgStore {
    path: PathBuf,
}

impl CkgStore {
    /// Create the store, building the parent directory and schema eagerly.
    ///
    /// Mirrors `ckg_store.py::__init__` (parent `mkdir(parents=True)` +
    /// `_init_schema`).
    pub fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dir for {}", path.display()))?;
        }
        let store = CkgStore {
            path: path.to_path_buf(),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Path to the underlying SQLite database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Open a fresh connection with the WAL + busy_timeout pragmas applied.
    ///
    /// Mirrors `ckg_store.py::_connect` (a new connection per call, committed
    /// and closed by the caller via RAII drop).
    fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)
            .with_context(|| format!("opening ckg store at {}", self.path.display()))?;
        conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)
            .context("setting busy_timeout pragma")?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("setting journal_mode=WAL pragma")?;
        Ok(conn)
    }

    /// Run the full DDL once. Mirrors `ckg_store.py::_init_schema`.
    fn init_schema(&self) -> Result<()> {
        let conn = self.connect()?;
        conn.execute_batch(SCHEMA_DDL)
            .with_context(|| format!("initializing ckg schema at {}", self.path.display()))?;
        Ok(())
    }

    /// Full rebuild: wipe both tables then bulk-insert.
    ///
    /// Mirrors `ckg_store.py::replace_all` (one transaction: DELETE both tables
    /// then INSERT OR REPLACE all rows).
    pub fn replace_all(&self, nodes: &[CkgNodeRow], edges: &[CkgEdgeRow]) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction().context("beginning replace_all transaction")?;
        tx.execute("DELETE FROM ckg_nodes", params![])
            .context("deleting all ckg_nodes")?;
        tx.execute("DELETE FROM ckg_edges", params![])
            .context("deleting all ckg_edges")?;
        Self::insert(&tx, nodes, edges)?;
        tx.commit().context("committing replace_all transaction")?;
        Ok(())
    }

    /// Incremental delta: drop every node/edge belonging to `files` (nodes by
    /// `file`, edges by `src_file`) then insert the supplied rows.
    ///
    /// Mirrors `ckg_store.py::replace_for_files` (one transaction).
    pub fn replace_for_files(
        &self,
        files: &[String],
        nodes: &[CkgNodeRow],
        edges: &[CkgEdgeRow],
    ) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction().context("beginning replace_for_files transaction")?;
        if !files.is_empty() {
            let placeholders: Vec<&str> = vec!["?"; files.len()];
            let del_nodes = format!("DELETE FROM ckg_nodes WHERE file IN ({})", placeholders.join(","));
            let del_edges = format!("DELETE FROM ckg_edges WHERE src_file IN ({})", placeholders.join(","));
            let params: Vec<&dyn ToSql> = files.iter().map(|s| s as &dyn ToSql).collect();
            tx.execute(&del_nodes, params_from_iter(params.clone()))
                .context("deleting ckg_nodes by file")?;
            tx.execute(&del_edges, params_from_iter(params))
                .context("deleting ckg_edges by src_file")?;
        }
        Self::insert(&tx, nodes, edges)?;
        tx.commit().context("committing replace_for_files transaction")?;
        Ok(())
    }

    /// Bulk-insert rows inside the (already-open) connection. Mirrors
    /// `ckg_store.py::_insert`.
    fn insert(conn: &Connection, nodes: &[CkgNodeRow], edges: &[CkgEdgeRow]) -> Result<()> {
        if !nodes.is_empty() {
            let mut stmt = conn
                .prepare(
                    "INSERT OR REPLACE INTO ckg_nodes \
                     (id, kind, name, file, start_line, end_line, lang) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .context("preparing ckg_nodes insert")?;
            for n in nodes {
                stmt.execute(params![
                    &n.id,
                    &n.kind,
                    &n.name,
                    &n.file,
                    &n.start_line,
                    &n.end_line,
                    &n.lang,
                ])
                .with_context(|| format!("inserting ckg node {}", n.id))?;
            }
            drop(stmt);
        }
        if !edges.is_empty() {
            let mut stmt = conn
                .prepare(
                    "INSERT OR REPLACE INTO ckg_edges (src, dst, kind, src_file) \
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .context("preparing ckg_edges insert")?;
            for e in edges {
                stmt.execute(params![&e.src, &e.dst, &e.kind, &e.src_file])
                    .with_context(|| format!("inserting ckg edge {} -> {}", e.src, e.dst))?;
            }
            drop(stmt);
        }
        Ok(())
    }
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn unique_temp_db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ckg-store-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("ckg.sqlite")
    }

    /// Port of `oracle/store/ckg_store.py::test_ckg_store_roundtrip`.
    ///
    /// Verifies: `replace_all` → row read via plain SELECT, then
    /// `replace_for_files` wipes the neighborhood back to empty.
    #[test]
    fn test_ckg_store_roundtrip() {
        let db = unique_temp_db("roundtrip");
        let store = CkgStore::new(&db).expect("create store");

        let nodes = vec![
            CkgNodeRow {
                id: "f.py".to_string(),
                kind: "FILE".to_string(),
                name: None,
                file: "f.py".to_string(),
                start_line: Some(1),
                end_line: Some(10),
                lang: Some("Python".to_string()),
            },
            CkgNodeRow {
                id: "f.py#2-3-0".to_string(),
                kind: "function_definition".to_string(),
                name: Some("foo".to_string()),
                file: "f.py".to_string(),
                start_line: Some(2),
                end_line: Some(3),
                lang: Some("Python".to_string()),
            },
            // Node with ALL optional fields None — verifies NULL roundtrip.
            CkgNodeRow {
                id: "g.py".to_string(),
                kind: "FILE".to_string(),
                name: None,
                file: "g.py".to_string(),
                start_line: None,
                end_line: Some(5),
                lang: None,
            },
        ];
        let edges = vec![CkgEdgeRow {
            src: "f.py".to_string(),
            dst: "f.py#2-3-0".to_string(),
            kind: "CONTAIN".to_string(),
            src_file: "f.py".to_string(),
        }];

        store.replace_all(&nodes, &edges).expect("replace_all");

        // Read back via a plain SELECT (mirrors the Python `get_neighborhood`
        // recursive-CTE path; here we exercise the schema + roundtrip contract).
        let conn = store.connect().expect("connect");
        let rows: Vec<(String, String, String, String)> = conn
            .prepare("SELECT src, dst, kind, src_file FROM ckg_edges WHERE src = ?1")
            .expect("prepare")
            .query_map(["f.py"], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("collect");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "f.py#2-3-0");

        // Read back the fully-None optional-field node and assert SQL NULL → None.
        let none_node: Option<(String, Option<i64>, Option<String>, Option<String>)> = conn
            .prepare("SELECT id, start_line, name, lang FROM ckg_nodes WHERE id = ?1")
            .expect("prepare")
            .query_map(["g.py"], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect")
            .into_iter()
            .next();
        assert!(none_node.is_some(), "g.py node should be present after replace_all");
        let (id, start_line, name, lang) = none_node.unwrap();
        assert_eq!(id, "g.py");
        assert_eq!(start_line, None, "start_line should be SQL NULL → None");
        assert_eq!(name, None, "name should be SQL NULL → None");
        assert_eq!(lang, None, "lang should be SQL NULL → None");

        // `replace_for_files` wipes the neighborhood back to empty.
        store
            .replace_for_files(&["f.py".to_string()], &[], &[])
            .expect("replace_for_files");

        let empty: Vec<(String, String, String, String)> = conn
            .prepare("SELECT src, dst, kind, src_file FROM ckg_edges WHERE src = ?1")
            .expect("prepare")
            .query_map(["f.py"], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("collect");
        assert!(empty.is_empty());

        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    /// Assert the exact schema: table and index names must match the Python
    /// `ckg_store.py::_init_schema` verbatim — `aspis_mcp.py` reads this DB
    /// with the Python CkgStore, so any drift is a wire-break.
    #[test]
    fn test_ckg_schema_matches_python() {
        let db = unique_temp_db("schema");
        let store = CkgStore::new(&db).expect("create store");

        let conn = store.connect().expect("connect");
        let rows: Vec<(String, String, String)> = conn
            .prepare(
                "SELECT type, name, tbl_name FROM sqlite_master \
                 WHERE tbl_name IN ('ckg_nodes','ckg_edges') \
                    OR type IN ('table','index') \
                 ORDER BY type, name",
            )
            .expect("prepare")
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("collect");

        let tables: Vec<&str> = rows
            .iter()
            .filter(|r| r.0 == "table")
            .map(|r| r.2.as_str())
            .collect();

        // Two tables.
        assert_eq!(tables.iter().filter(|t| **t == "ckg_nodes").count(), 1);
        assert_eq!(tables.iter().filter(|t| **t == "ckg_edges").count(), 1);

        // Five indexes (two on ckg_nodes, three on ckg_edges); exclude sqlite_autoindex
        // entries generated for the PRIMARY KEY.
        let node_indexes: Vec<&str> = rows
            .iter()
            .filter(|r| r.0 == "index" && r.2 == "ckg_nodes" && !r.1.starts_with("sqlite_autoindex"))
            .map(|r| r.1.as_str())
            .collect();
        let edge_indexes: Vec<&str> = rows
            .iter()
            .filter(|r| r.0 == "index" && r.2 == "ckg_edges" && !r.1.starts_with("sqlite_autoindex"))
            .map(|r| r.1.as_str())
            .collect();
        assert_eq!(node_indexes, vec!["idx_ckg_nodes_file", "idx_ckg_nodes_name"]);
        assert_eq!(
            edge_indexes,
            vec![
                "idx_ckg_edges_dst",
                "idx_ckg_edges_src",
                "idx_ckg_edges_srcfile"
            ]
        );

        // PRIMARY KEYs: ckg_nodes.id TEXT PRIMARY KEY, ckg_edges(src,dst,kind).
        let pk_nodes: String = conn
            .query_row_and_then(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='ckg_nodes'",
                [],
                |r| r.get::<_, String>(0),
            )
            .expect("pk nodes");
        assert!(pk_nodes.contains("PRIMARY KEY"));
        assert!(pk_nodes.contains("id TEXT"));

        let pk_edges: String = conn
            .query_row_and_then(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='ckg_edges'",
                [],
                |r| r.get::<_, String>(0),
            )
            .expect("pk edges");
        assert!(pk_edges.contains("PRIMARY KEY"));
        assert!(pk_edges.contains("src TEXT"));
        assert!(pk_edges.contains("dst TEXT"));
        assert!(pk_edges.contains("kind TEXT"));

        // Index column lists (the `ON <table>(...)` part). Python's `ckg_store.py`
        // creates these exact indexes; any drift in column list is a wire-break.
        let mut stmt = conn
            .prepare(
                "SELECT name, sql FROM sqlite_master \
                 WHERE type='index' \
                   AND tbl_name IN ('ckg_nodes','ckg_edges') \
                   AND name NOT LIKE 'sqlite_autoindex%' \
                 ORDER BY name",
            )
            .expect("prepare index sql query");
        let index_sql: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("index sql query")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect index sql");
        let idx_map: std::collections::HashMap<&str, &str> =
            index_sql.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
        assert!(idx_map.get("idx_ckg_nodes_file")
            .map(|s| s.contains("(file)"))
            .unwrap_or(false),
            "idx_ckg_nodes_file should index (file)");
        assert!(idx_map.get("idx_ckg_nodes_name")
            .map(|s| s.contains("(name)"))
            .unwrap_or(false),
            "idx_ckg_nodes_name should index (name)");
        assert!(idx_map.get("idx_ckg_edges_dst")
            .map(|s| s.contains("(dst, kind)"))
            .unwrap_or(false),
            "idx_ckg_edges_dst should index (dst, kind)");
        assert!(idx_map.get("idx_ckg_edges_src")
            .map(|s| s.contains("(src, kind)"))
            .unwrap_or(false),
            "idx_ckg_edges_src should index (src, kind)");
        assert!(idx_map.get("idx_ckg_edges_srcfile")
            .map(|s| s.contains("(src_file)"))
            .unwrap_or(false),
            "idx_ckg_edges_srcfile should index (src_file)");

        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }
}
