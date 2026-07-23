//! SQLite-backed metadata store for the Oracle runtime.
//!
//! Port of `oracle/store/sqlite_store.py::SQLiteStore`. The lifecycle matches
//! the Python original exactly: the constructor creates the parent directory and
//! eagerly builds the schema (`CREATE TABLE IF NOT EXISTS`); every public method
//! opens its own connection, applies the WAL + busy_timeout pragmas, commits,
//! and drops (closes) the connection. Array-valued columns are stored as
//! JSON text (`""`/`NULL` → `[]` on read); `symbols_used` is deserialized
//! with the three-way (string / list / missing) handling from the Python
//! `_deserialize_chunk`.

use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Busy timeout in milliseconds, mirrored from `sqlite_store.py::_BUSY_TIMEOUT_MS`.
const BUSY_TIMEOUT_MS: i32 = 5000;

/// Array-valued node-card columns (stored as JSON text).
/// Mirrors `sqlite_store.py::ARRAY_FIELDS`.
pub const ARRAY_FIELDS: &[&str] = &["espone_api", "dipende_da", "simile_a", "tecnologie"];

/// Exact DDL (every statement `IF NOT EXISTS`), byte-equivalent to
/// `sqlite_store.py::_init_schema` §1.3.
const SCHEMA_DDL: &str = "
CREATE TABLE IF NOT EXISTS node_cards (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  area TEXT NOT NULL,
  cluster_semantic TEXT NOT NULL,
  funzione_primaria TEXT NOT NULL,
  espone_api TEXT NOT NULL,
  dipende_da TEXT NOT NULL,
  simile_a TEXT NOT NULL,
  tecnologie TEXT NOT NULL,
  file_sorgente TEXT NOT NULL,
  ultima_modifica TEXT NOT NULL,
  source TEXT NOT NULL,
  embedding_dims INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_node_area ON node_cards(area);
CREATE INDEX IF NOT EXISTS idx_node_cluster ON node_cards(cluster_semantic);
CREATE INDEX IF NOT EXISTS idx_node_label ON node_cards(label);

CREATE TABLE IF NOT EXISTS file_chunks (
  id TEXT PRIMARY KEY,
  file_id TEXT NOT NULL,
  chunk_index INTEGER NOT NULL,
  start_char INTEGER NOT NULL,
  end_char INTEGER NOT NULL,
  text TEXT NOT NULL,
  file_sorgente TEXT NOT NULL,
  ultima_modifica TEXT NOT NULL,
  embedding_dims INTEGER NOT NULL,
  kind TEXT NOT NULL DEFAULT '',
  symbol_name TEXT NOT NULL DEFAULT '',
  signature TEXT NOT NULL DEFAULT '',
  line_start INTEGER NOT NULL DEFAULT 0,
  line_end INTEGER NOT NULL DEFAULT 0,
  language TEXT NOT NULL DEFAULT '',
  symbols_used TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_chunk_file ON file_chunks(file_id);

CREATE TABLE IF NOT EXISTS file_clusters (
  file_id TEXT PRIMARY KEY,
  cluster_id INTEGER NOT NULL,
  score REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS clusters_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
";

/// A node-card row (`node_cards` table).
///
/// Mirrors the Python dict keys verbatim, including the Italian field names.
/// Array fields are typed `Vec<String>` (stored as JSON text in SQLite).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeCard {
    pub id: String,
    pub label: String,
    pub area: String,
    pub cluster_semantic: String,
    pub funzione_primaria: String,
    pub espone_api: Vec<String>,
    pub dipende_da: Vec<String>,
    pub simile_a: Vec<String>,
    pub tecnologie: Vec<String>,
    pub file_sorgente: String,
    pub ultima_modifica: String,
    pub source: String,
    pub embedding_dims: i64,
}

/// A chunk row (`file_chunks` table).
///
/// Field names mirror the SQLite columns verbatim. `symbols_used` is typed
/// `Vec<String>` (JSON text on disk).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileChunk {
    pub id: String,
    pub file_id: String,
    pub chunk_index: i64,
    pub start_char: i64,
    pub end_char: i64,
    pub text: String,
    pub file_sorgente: String,
    pub ultima_modifica: String,
    pub embedding_dims: i64,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub line_start: i64,
    pub line_end: i64,
    pub language: String,
    pub symbols_used: Vec<String>,
}

/// A file-cluster membership row (`file_clusters` table).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileCluster {
    pub file_id: String,
    pub cluster_id: i64,
    pub score: f64,
}

/// SQLite metadata store. Mirrors `oracle/store/sqlite_store.py::SQLiteStore`.
pub struct SqliteStore {
    path: PathBuf,
}

impl SqliteStore {
    /// Create the store, building the parent directory and schema eagerly.
    ///
    /// Mirrors `sqlite_store.py::__init__` (parent `mkdir(parents=True)` +
    /// `_init_schema`).
    pub fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dir for {}", path.display()))?;
        }
        let store = SqliteStore {
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
    /// Mirrors `sqlite_store.py::_connect` (a new connection per call, committed
    /// and closed by the caller via RAII drop).
    fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)
            .with_context(|| format!("opening sqlite store at {}", self.path.display()))?;
        conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)
            .context("setting busy_timeout pragma")?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("setting journal_mode=WAL pragma")?;
        Ok(conn)
    }

    /// Run the full DDL once. Mirrors `sqlite_store.py::_init_schema`.
    fn init_schema(&self) -> Result<()> {
        let conn = self.connect()?;
        conn.execute_batch(SCHEMA_DDL)
            .with_context(|| format!("initializing schema at {}", self.path.display()))?;
        Ok(())
    }

    /// Column names of a table via `PRAGMA table_info` (used by tests/inspection).
    pub fn table_columns(&self, table: &str) -> Result<Vec<String>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info(?)")
            .context("preparing pragma table_info")?;
        let names: Vec<String> = stmt
            .query_map([table], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(names)
    }

    // ── node_cards ────────────────────────────────────────────────────────

    /// Insert or update many node cards (UPSERT on `id`).
    ///
    /// Mirrors `sqlite_store.py::upsert_many` (`INSERT … ON CONFLICT(id) DO
    /// UPDATE`, every column set to `excluded.*`). The four array fields are
    /// JSON-encoded before write.
    pub fn upsert_many(&self, cards: &[NodeCard]) -> Result<()> {
        let conn = self.connect()?;
        // One explicit transaction for the whole batch — the per-row loop
        // otherwise pays an implicit-commit fsync per card, where Python's
        // single executemany runs inside one commit.
        conn.execute_batch("BEGIN")
            .context("beginning upsert_many transaction")?;
        let sql = "
            INSERT INTO node_cards (
              id, label, area, cluster_semantic, funzione_primaria, espone_api,
              dipende_da, simile_a, tecnologie, file_sorgente, ultima_modifica,
              source, embedding_dims
            ) VALUES (
              ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
            )
            ON CONFLICT(id) DO UPDATE SET
              label=excluded.label,
              area=excluded.area,
              cluster_semantic=excluded.cluster_semantic,
              funzione_primaria=excluded.funzione_primaria,
              espone_api=excluded.espone_api,
              dipende_da=excluded.dipende_da,
              simile_a=excluded.simile_a,
              tecnologie=excluded.tecnologie,
              file_sorgente=excluded.file_sorgente,
              ultima_modifica=excluded.ultima_modifica,
              source=excluded.source,
              embedding_dims=excluded.embedding_dims
        ";
        let mut stmt = conn.prepare(sql).context("preparing upsert_many")?;
        for card in cards {
            let espone = json_arr(&card.espone_api);
            let dipende = json_arr(&card.dipende_da);
            let simile = json_arr(&card.simile_a);
            let tecnologie = json_arr(&card.tecnologie);
            let params: Vec<&dyn ToSql> = vec![
                &card.id,
                &card.label,
                &card.area,
                &card.cluster_semantic,
                &card.funzione_primaria,
                &espone,
                &dipende,
                &simile,
                &tecnologie,
                &card.file_sorgente,
                &card.ultima_modifica,
                &card.source,
                &card.embedding_dims,
            ];
            stmt.execute(params_from_iter(params))
                .with_context(|| format!("upserting node card {}", card.id))?;
        }
        drop(stmt);
        conn.execute_batch("COMMIT")
            .context("committing upsert_many transaction")?;
        Ok(())
    }

    /// Delete all node cards, then upsert the given cards (two transactions).
    ///
    /// Mirrors `sqlite_store.py::replace_all` (separate `DELETE` commit, then
    /// `upsert_many` commit).
    pub fn replace_all(&self, cards: &[NodeCard]) -> Result<()> {
        {
            let conn = self.connect()?;
            conn.execute("DELETE FROM node_cards", params![])
                .context("deleting all node cards")?;
        }
        self.upsert_many(cards)
    }

    /// Delete the given node ids. No-op (early return) when the list is empty.
    ///
    /// Mirrors `sqlite_store.py::delete_nodes`.
    pub fn delete_nodes(&self, node_ids: &[String]) -> Result<()> {
        if node_ids.is_empty() {
            return Ok(());
        }
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("DELETE FROM node_cards WHERE id = ?")
            .context("preparing delete_nodes")?;
        for id in node_ids {
            stmt.execute([id])
                .with_context(|| format!("deleting node {id}"))?;
        }
        Ok(())
    }

    /// Fetch a single node card by id, or `None` on miss.
    ///
    /// Mirrors `sqlite_store.py::get_node`.
    pub fn get_node(&self, node_id: &str) -> Result<Option<NodeCard>> {
        let conn = self.connect()?;
        let row = conn
            .query_row_and_then(
                "SELECT * FROM node_cards WHERE id = ?",
                [node_id],
                row_to_node,
            )
            .optional()
            .context("querying node card")?;
        Ok(row)
    }

    /// All node cards ordered by `id`.
    ///
    /// Mirrors `sqlite_store.py::all_nodes` (`ORDER BY id`).
    pub fn all_nodes(&self) -> Result<Vec<NodeCard>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT * FROM node_cards ORDER BY id")
            .context("preparing all_nodes")?;
        let empty: &[&dyn ToSql] = &[];
        let rows = stmt
            .query_map(empty, row_to_node)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Node cards whose `cluster_semantic` matches (case-insensitive).
    ///
    /// Mirrors `sqlite_store.py::by_cluster` (loads all, filters in memory).
    pub fn by_cluster(&self, cluster: &str) -> Result<Vec<NodeCard>> {
        let target = cluster.to_lowercase();
        Ok(self
            .all_nodes()?
            .into_iter()
            .filter(|n| n.cluster_semantic.to_lowercase() == target)
            .collect())
    }

    /// Node cards whose `area` matches (case-insensitive).
    ///
    /// Mirrors `sqlite_store.py::by_area` (loads all, filters in memory).
    pub fn by_area(&self, area: &str) -> Result<Vec<NodeCard>> {
        let target = area.to_lowercase();
        Ok(self
            .all_nodes()?
            .into_iter()
            .filter(|n| n.area.to_lowercase() == target)
            .collect())
    }

    /// Total node-card count. Mirrors `sqlite_store.py::count`.
    pub fn count(&self) -> Result<usize> {
        let conn = self.connect()?;
        let n: i64 = conn
            .query_row_and_then("SELECT COUNT(*) FROM node_cards", params![], |r| r.get(0))
            .context("counting node cards")?;
        Ok(n as usize)
    }

    // ── file_chunks ───────────────────────────────────────────────────────

    /// Replace all chunks for the given file ids (single transaction):
    /// delete the old chunks, then insert/upsert the new ones.
    ///
    /// Mirrors `sqlite_store.py::replace_chunks_for_files`.
    pub fn replace_chunks_for_files(
        &self,
        file_ids: &[String],
        chunks: &[FileChunk],
    ) -> Result<()> {
        let conn = self.connect()?;
        // Python runs delete + insert inside ONE commit (`with self._connect()`);
        // an explicit transaction keeps the same crash-atomicity (a failure can
        // never leave chunks deleted but not re-inserted).
        conn.execute_batch("BEGIN")
            .context("beginning replace_chunks_for_files transaction")?;
        {
            let mut del = conn
                .prepare("DELETE FROM file_chunks WHERE file_id = ?")
                .context("preparing chunk delete")?;
            for fid in file_ids {
                del.execute([fid])
                    .with_context(|| format!("deleting chunks for {fid}"))?;
            }
        }
        if chunks.is_empty() {
            conn.execute_batch("COMMIT")
                .context("committing replace_chunks_for_files transaction")?;
            return Ok(());
        }
        let mut stmt = conn
            .prepare(
                "
            INSERT INTO file_chunks (
              id, file_id, chunk_index, start_char, end_char, text,
              file_sorgente, ultima_modifica, embedding_dims,
              kind, symbol_name, signature, line_start, line_end, language, symbols_used
            ) VALUES (
              ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
            )
            ON CONFLICT(id) DO UPDATE SET
              file_id=excluded.file_id,
              chunk_index=excluded.chunk_index,
              start_char=excluded.start_char,
              end_char=excluded.end_char,
              text=excluded.text,
              file_sorgente=excluded.file_sorgente,
              ultima_modifica=excluded.ultima_modifica,
              embedding_dims=excluded.embedding_dims,
              kind=excluded.kind,
              symbol_name=excluded.symbol_name,
              signature=excluded.signature,
              line_start=excluded.line_start,
              line_end=excluded.line_end,
              language=excluded.language,
              symbols_used=excluded.symbols_used
        ",
            )
            .context("preparing chunk upsert")?;
        for c in chunks {
            let symbols = json_arr(&c.symbols_used);
            let params: Vec<&dyn ToSql> = vec![
                &c.id,
                &c.file_id,
                &c.chunk_index,
                &c.start_char,
                &c.end_char,
                &c.text,
                &c.file_sorgente,
                &c.ultima_modifica,
                &c.embedding_dims,
                &c.kind,
                &c.symbol_name,
                &c.signature,
                &c.line_start,
                &c.line_end,
                &c.language,
                &symbols,
            ];
            stmt.execute(params_from_iter(params))
                .with_context(|| format!("upserting chunk {}", c.id))?;
        }
        drop(stmt);
        conn.execute_batch("COMMIT")
            .context("committing replace_chunks_for_files transaction")?;
        Ok(())
    }

    /// Delete all chunks, then (if any) replace them (two transactions when
    /// non-empty). Mirrors `sqlite_store.py::replace_all_chunks`.
    pub fn replace_all_chunks(&self, chunks: &[FileChunk]) -> Result<()> {
        {
            let conn = self.connect()?;
            conn.execute("DELETE FROM file_chunks", params![])
                .context("deleting all chunks")?;
        }
        if chunks.is_empty() {
            return Ok(());
        }
        let mut file_ids: Vec<String> = chunks.iter().map(|c| c.file_id.clone()).collect();
        file_ids.sort();
        file_ids.dedup();
        self.replace_chunks_for_files(&file_ids, chunks)
    }

    /// Chunk ids for the given file ids (empty input → empty result).
    ///
    /// Mirrors `sqlite_store.py::chunk_ids_for_files` (uses `?` placeholders).
    pub fn chunk_ids_for_files(&self, file_ids: &[String]) -> Result<Vec<String>> {
        if file_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.connect()?;
        let placeholders = vec!["?"; file_ids.len()].join(",");
        let sql = format!("SELECT id FROM file_chunks WHERE file_id IN ({placeholders})");
        let mut stmt = conn
            .prepare(&sql)
            .context("preparing chunk_ids_for_files")?;
        let ids = stmt
            .query_map(params_from_iter(file_ids.iter()), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// All chunks for a file ordered by `chunk_index`.
    ///
    /// Mirrors `sqlite_store.py::chunks_for_file` (`ORDER BY chunk_index`).
    pub fn chunks_for_file(&self, file_id: &str) -> Result<Vec<FileChunk>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT * FROM file_chunks WHERE file_id = ? ORDER BY chunk_index")
            .context("preparing chunks_for_file")?;
        let rows = stmt
            .query_map([file_id], |row| row_to_chunk(row, file_id))
            .context("querying chunks_for_file")?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Fetch a single chunk by id, or `None` on miss.
    ///
    /// Mirrors `sqlite_store.py::get_chunk`.
    pub fn get_chunk(&self, chunk_id: &str) -> Result<Option<FileChunk>> {
        let conn = self.connect()?;
        let row = conn
            .query_row_and_then(
                "SELECT * FROM file_chunks WHERE id = ?",
                [chunk_id],
                |row| row_to_chunk(row, chunk_id),
            )
            .optional()
            .context("querying chunk")?;
        Ok(row)
    }

    /// All chunks ordered by `id`. Mirrors `sqlite_store.py::all_chunks`.
    pub fn all_chunks(&self) -> Result<Vec<FileChunk>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT * FROM file_chunks ORDER BY id")
            .context("preparing all_chunks")?;
        let empty: &[&dyn ToSql] = &[];
        let rows = stmt
            .query_map(empty, |row| row_to_chunk(row, ""))
            .context("querying all_chunks")?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Bounded chunk materialization for lexical scoring — never loads more
    /// than `limit` rows (ordered by `id`). Used as an OOM guard on the
    /// query path; callers that need the full corpus use [`all_chunks`].
    pub fn all_chunks_limited(&self, limit: usize) -> Result<Vec<FileChunk>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT * FROM file_chunks ORDER BY id LIMIT ?1")
            .context("preparing all_chunks_limited")?;
        let rows = stmt
            .query_map(params![limit as i64], |row| row_to_chunk(row, ""))
            .context("querying all_chunks_limited")?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Total chunk count. Mirrors `sqlite_store.py::chunk_count`.
    pub fn chunk_count(&self) -> Result<usize> {
        let conn = self.connect()?;
        let n: i64 = conn
            .query_row_and_then("SELECT COUNT(*) FROM file_chunks", params![], |r| r.get(0))
            .context("counting chunks")?;
        Ok(n as usize)
    }

    /// Distinct files that have at least one chunk.
    /// Mirrors `sqlite_store.py::chunk_file_count`.
    pub fn chunk_file_count(&self) -> Result<usize> {
        let conn = self.connect()?;
        let n: i64 = conn
            .query_row_and_then(
                "SELECT COUNT(DISTINCT file_id) FROM file_chunks",
                params![],
                |r| r.get(0),
            )
            .context("counting chunk files")?;
        Ok(n as usize)
    }

    // ── file_clusters / clusters_meta ─────────────────────────────────────

    /// Replace all file-cluster rows (and optionally the `epoch` meta) in a
    /// SINGLE transaction. Mirrors `sqlite_store.py::replace_file_clusters`.
    pub fn replace_file_clusters(&self, rows: &[FileCluster], epoch: Option<&str>) -> Result<()> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM file_clusters", params![])
            .context("deleting file_clusters")?;
        if !rows.is_empty() {
            let mut stmt = conn
                .prepare(
                    "INSERT INTO file_clusters (file_id, cluster_id, score) \
                     VALUES (?1, ?2, ?3)",
                )
                .context("preparing file_clusters insert")?;
            for r in rows {
                let params: Vec<&dyn ToSql> = vec![&r.file_id, &r.cluster_id, &r.score];
                stmt.execute(params_from_iter(params))
                    .with_context(|| format!("inserting cluster for {}", r.file_id))?;
            }
        }
        if let Some(epoch) = epoch {
            conn.execute(
                "INSERT OR REPLACE INTO clusters_meta (key, value) VALUES ('epoch', ?1)",
                [epoch],
            )
            .context("writing clusters epoch")?;
        }
        Ok(())
    }

    /// Set the `epoch` meta value. Mirrors `sqlite_store.py::set_clusters_epoch`.
    pub fn set_clusters_epoch(&self, epoch: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT OR REPLACE INTO clusters_meta (key, value) VALUES ('epoch', ?1)",
            [epoch],
        )
        .context("writing clusters epoch")?;
        Ok(())
    }

    /// Read the `epoch` meta value, or `None` if absent.
    /// Mirrors `sqlite_store.py::get_clusters_epoch`.
    pub fn get_clusters_epoch(&self) -> Result<Option<String>> {
        let conn = self.connect()?;
        let row = conn
            .query_row_and_then(
                "SELECT value FROM clusters_meta WHERE key = 'epoch'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .context("reading clusters epoch")?;
        Ok(row)
    }

    /// All file-cluster rows ordered by `cluster_id, file_id`.
    /// Mirrors `sqlite_store.py::get_file_clusters`.
    pub fn get_file_clusters(&self) -> Result<Vec<FileCluster>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT file_id, cluster_id, score FROM file_clusters \
                 ORDER BY cluster_id, file_id",
            )
            .context("preparing get_file_clusters")?;
        let empty: &[&dyn ToSql] = &[];
        let rows = stmt
            .query_map(empty, row_to_cluster)
            .context("querying file_clusters")?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// File-cluster rows for one cluster, ordered by `score DESC, file_id`.
    /// Mirrors `sqlite_store.py::get_cluster_members`.
    pub fn get_cluster_members(&self, cluster_id: i64) -> Result<Vec<FileCluster>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT file_id, cluster_id, score FROM file_clusters \
                 WHERE cluster_id = ? ORDER BY score DESC, file_id",
            )
            .context("preparing get_cluster_members")?;
        let rows = stmt
            .query_map([cluster_id], row_to_cluster)
            .context("querying cluster members")?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

// ── row mappers ────────────────────────────────────────────────────────────

/// Deserialize a JSON array column, with the `""`/`NULL` → `[]` fallback.
/// Mirrors `sqlite_store.py::_deserialize` (`json.loads(out[field] or "[]")`).
fn deser_arr(s: Option<String>) -> Vec<String> {
    match s {
        None => Vec::new(),
        Some(s) if s.is_empty() => Vec::new(),
        Some(s) => serde_json::from_str(&s).unwrap_or_default(),
    }
}

/// Deserialize `symbols_used` with the three-way handling from
/// `sqlite_store.py::_deserialize_chunk` (string → parse, missing/empty → `[]`,
/// malformed JSON → `[]`).
fn deser_symbols(s: Option<String>) -> Vec<String> {
    match s {
        None => Vec::new(),
        Some(s) if s.is_empty() => Vec::new(),
        Some(s) => serde_json::from_str(&s).unwrap_or_default(),
    }
}

/// JSON-encode an array field for storage (never fails; falls back to `[]`).
/// Mirrors `sqlite_store.py::_serialize` (`json.dumps(out.get(field, [])))`).
fn json_arr(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}

/// Map a `node_cards` row to a `NodeCard`.
fn row_to_node(row: &rusqlite::Row) -> rusqlite::Result<NodeCard> {
    Ok(NodeCard {
        id: row.get("id")?,
        label: row.get("label")?,
        area: row.get("area")?,
        cluster_semantic: row.get("cluster_semantic")?,
        funzione_primaria: row.get("funzione_primaria")?,
        espone_api: deser_arr(row.get::<_, Option<String>>("espone_api").ok().flatten()),
        dipende_da: deser_arr(row.get::<_, Option<String>>("dipende_da").ok().flatten()),
        simile_a: deser_arr(row.get::<_, Option<String>>("simile_a").ok().flatten()),
        tecnologie: deser_arr(row.get::<_, Option<String>>("tecnologie").ok().flatten()),
        file_sorgente: row.get("file_sorgente")?,
        ultima_modifica: row.get("ultima_modifica")?,
        source: row.get("source")?,
        embedding_dims: row.get("embedding_dims")?,
    })
}

/// Map a `file_chunks` row to a `FileChunk`. `self_id` is only used for
/// diagnostics. Mirrors `sqlite_store.py::_deserialize_chunk`.
fn row_to_chunk(row: &rusqlite::Row, _self_id: &str) -> rusqlite::Result<FileChunk> {
    Ok(FileChunk {
        id: row.get("id")?,
        file_id: row.get("file_id")?,
        chunk_index: row.get("chunk_index")?,
        start_char: row.get("start_char")?,
        end_char: row.get("end_char")?,
        text: row.get("text")?,
        file_sorgente: row.get("file_sorgente")?,
        ultima_modifica: row.get("ultima_modifica")?,
        embedding_dims: row.get("embedding_dims")?,
        kind: row.get("kind")?,
        symbol_name: row.get("symbol_name")?,
        signature: row.get("signature")?,
        line_start: row.get("line_start")?,
        line_end: row.get("line_end")?,
        language: row.get("language")?,
        symbols_used: deser_symbols(row.get::<_, Option<String>>("symbols_used").ok().flatten()),
    })
}

/// Map a `file_clusters` row to a `FileCluster`.
fn row_to_cluster(row: &rusqlite::Row) -> rusqlite::Result<FileCluster> {
    Ok(FileCluster {
        file_id: row.get(0)?,
        cluster_id: row.get(1)?,
        score: row.get(2)?,
    })
}
