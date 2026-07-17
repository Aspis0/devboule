//! Store-layer integration & unit tests for `oracle-core`.
//!
//! Covers the SQLite metadata store, the LanceDB vector store and the
//! chunk-index manifest. Mirrors the behavioral spec in `p1-recon-spec.md`.

use oracle_core::config::active_chunk_profile_version;
use oracle_core::store::lance::{hash_embed, LanceRow, LanceStore};
use oracle_core::store::manifest::{
    file_needs_index, load_manifest, manifest_files_for_root, strip_verbatim_prefix,
    text_chunks_up_to_date, ManifestFileEntry,
};
use oracle_core::store::sqlite::{FileChunk, NodeCard, SqliteStore};
use std::collections::HashMap;
use std::path::Path;
use tempfile::tempdir;

// ── helpers ────────────────────────────────────────────────────────────────

fn node(id: &str) -> NodeCard {
    NodeCard {
        id: id.to_string(),
        label: format!("Label {id}"),
        area: "server".to_string(),
        cluster_semantic: "api".to_string(),
        funzione_primaria: "gestisce la richiesta".to_string(),
        espone_api: vec!["GET /x".to_string()],
        dipende_da: vec![],
        simile_a: vec!["n8".to_string(), "nöde-2".to_string()],
        tecnologie: vec!["caffè".to_string()],
        file_sorgente: "src/mod.rs".to_string(),
        ultima_modifica: "2025-01-01T00:00:00Z".to_string(),
        source: "ckg".to_string(),
        embedding_dims: 1024,
    }
}

fn chunk(id: &str, file_id: &str) -> FileChunk {
    FileChunk {
        id: id.to_string(),
        file_id: file_id.to_string(),
        chunk_index: 0,
        start_char: 0,
        end_char: 10,
        text: "fn main() {}".to_string(),
        file_sorgente: "src/main.rs".to_string(),
        ultima_modifica: "2025-01-01T00:00:00Z".to_string(),
        embedding_dims: 1024,
        kind: "code".to_string(),
        symbol_name: "main".to_string(),
        signature: "fn main()".to_string(),
        line_start: 1,
        line_end: 2,
        language: "rust".to_string(),
        symbols_used: vec!["Vec".to_string()],
    }
}

// ── 1. SqliteStore round-trip ──────────────────────────────────────────

#[test]
fn sqlite_roundtrip_unicode_and_arrays() {
    let dir = tempdir().unwrap();
    let store = SqliteStore::new(&dir.path().join("metadata.sqlite")).unwrap();

    let cards = vec![node("n1"), node("n2")];
    store.upsert_many(&cards).unwrap();

    let got = store.get_node("n1").unwrap().expect("n1 present");
    assert_eq!(got, node("n1"));
    // Italian field + unicode array preserved.
    assert_eq!(got.funzione_primaria, "gestisce la richiesta");
    assert_eq!(got.tecnologie, vec!["caffè".to_string()]);
    assert_eq!(got.simile_a, vec!["n8".to_string(), "nöde-2".to_string()]);

    // all_nodes ordering.
    let all = store.all_nodes().unwrap();
    let ids: Vec<&str> = all.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["n1", "n2"]);

    // case-insensitive filters.
    assert_eq!(store.by_area("SERVER").unwrap().len(), 2);
    assert_eq!(store.by_cluster("API").unwrap().len(), 2);
    assert_eq!(store.by_cluster("other").unwrap().len(), 0);
    assert_eq!(store.count().unwrap(), 2);

    // empty-array guard: stores "[]", reads back empty Vec.
    let mut empty = node("n3");
    empty.espone_api = vec![];
    empty.simile_a = vec![];
    store.upsert_many(&[empty.clone()]).unwrap();
    let back = store.get_node("n3").unwrap().unwrap();
    assert!(back.espone_api.is_empty());
    assert!(back.simile_a.is_empty());
}

#[test]
fn sqlite_chunk_replace_and_malformed_symbols() {
    let dir = tempdir().unwrap();
    let store = SqliteStore::new(&dir.path().join("metadata.sqlite")).unwrap();

    store
        .replace_chunks_for_files(&["f.py".to_string()], &[chunk("f.py#chunk-0000", "f.py")])
        .unwrap();
    let chunks = store.chunks_for_file("f.py").unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].symbols_used, vec!["Vec".to_string()]);

    // replace_all_chunks (two transactions).
    store
        .replace_all_chunks(&[chunk("g.rs#chunk-0000", "g.rs")])
        .unwrap();
    assert_eq!(store.chunk_count().unwrap(), 1);
    assert_eq!(store.chunk_file_count().unwrap(), 1);
    assert_eq!(
        store.chunk_ids_for_files(&["g.rs".to_string()]).unwrap(),
        vec!["g.rs#chunk-0000".to_string()]
    );

    // malformed symbols_used JSON -> [].
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    conn.execute(
        "INSERT INTO file_chunks \
         (id, file_id, chunk_index, start_char, end_char, text, file_sorgente, \
          ultima_modifica, embedding_dims, symbols_used) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        rusqlite::params![
            "bad#chunk-0000",
            "bad",
            0,
            0,
            1,
            "x",
            "bad.py",
            "now",
            1024,
            "this is not json"
        ],
    )
    .unwrap();
    let bad = store.get_chunk("bad#chunk-0000").unwrap().unwrap();
    assert!(bad.symbols_used.is_empty(), "malformed symbols_used -> []");

    // empty-list guard for chunks.
    let mut c = chunk("e.rs#chunk-0000", "e.rs");
    c.symbols_used = vec![];
    store
        .replace_chunks_for_files(&["e.rs".to_string()], &[c])
        .unwrap();
    let e = store.chunks_for_file("e.rs").unwrap();
    assert!(e[0].symbols_used.is_empty());
}

#[test]
fn sqlite_file_clusters() {
    let dir = tempdir().unwrap();
    let store = SqliteStore::new(&dir.path().join("metadata.sqlite")).unwrap();
    store
        .replace_file_clusters(
            &[
                oracle_core::store::sqlite::FileCluster {
                    file_id: "a".to_string(),
                    cluster_id: 1,
                    score: 0.9,
                },
                oracle_core::store::sqlite::FileCluster {
                    file_id: "b".to_string(),
                    cluster_id: 1,
                    score: 0.5,
                },
            ],
            Some("epoch-42"),
        )
        .unwrap();
    let members = store.get_cluster_members(1).unwrap();
    assert_eq!(members.len(), 2);
    // ordered by score DESC, file_id.
    assert_eq!(members[0].file_id, "a");
    assert_eq!(members[1].file_id, "b");
    assert_eq!(store.get_clusters_epoch().unwrap().unwrap(), "epoch-42");

    let all = store.get_file_clusters().unwrap();
    assert_eq!(all.len(), 2);
}

// ── 2. DDL assertion ─────────────────────────────────────────────────────

#[test]
fn sqlite_ddl_columns() {
    let dir = tempdir().unwrap();
    let store = SqliteStore::new(&dir.path().join("metadata.sqlite")).unwrap();

    assert_eq!(
        store.table_columns("node_cards").unwrap(),
        vec![
            "id",
            "label",
            "area",
            "cluster_semantic",
            "funzione_primaria",
            "espone_api",
            "dipende_da",
            "simile_a",
            "tecnologie",
            "file_sorgente",
            "ultima_modifica",
            "source",
            "embedding_dims",
        ]
    );
    assert_eq!(
        store.table_columns("file_chunks").unwrap(),
        vec![
            "id",
            "file_id",
            "chunk_index",
            "start_char",
            "end_char",
            "text",
            "file_sorgente",
            "ultima_modifica",
            "embedding_dims",
            "kind",
            "symbol_name",
            "signature",
            "line_start",
            "line_end",
            "language",
            "symbols_used",
        ]
    );
    assert_eq!(
        store.table_columns("file_clusters").unwrap(),
        vec!["file_id", "cluster_id", "score"]
    );
    assert_eq!(
        store.table_columns("clusters_meta").unwrap(),
        vec!["key", "value"]
    );
}

// ── 3. Manifest decisions + verbatim stripping ────────────────────────────

#[test]
fn manifest_verbatim_stripping() {
    assert_eq!(
        strip_verbatim_prefix(r"\\?\C:\Users\test"),
        r"C:\Users\test"
    );
    assert_eq!(
        strip_verbatim_prefix(r"\\?\UNC\server\share"),
        r"\\server\share"
    );
    assert_eq!(strip_verbatim_prefix("/normal/path"), "/normal/path");
}

#[test]
fn manifest_needs_index_decisions() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let file = root.join("sample.py");
    std::fs::write(&file, b"print('hello')\n").unwrap();

    let store = SqliteStore::new(&dir.path().join("metadata.sqlite")).unwrap();
    let file_id = "sample.py";

    // (a) missing entry -> needs index.
    let mut files: HashMap<String, ManifestFileEntry> = HashMap::new();
    assert!(file_needs_index(&file, root, &files, &store).unwrap());

    // Build a "matching" entry from the current signature.
    let current = oracle_core::store::manifest::file_signature(&file, None).unwrap();
    let matching = ManifestFileEntry {
        size: current.size,
        mtime_ns: current.mtime_ns,
        updated_at: "2025-01-01T00:00:00Z".to_string(),
        chunks: Some(5),
        chunk_profile: Some(active_chunk_profile_version(None)),
    };
    files.insert(file_id.to_string(), matching.clone());

    // (b) size change -> needs index.
    let mut f = matching.clone();
    f.size += 1;
    let mut fs = HashMap::new();
    fs.insert(file_id.to_string(), f);
    assert!(file_needs_index(&file, root, &fs, &store).unwrap());

    // (c) mtime change -> needs index.
    let mut f = matching.clone();
    f.mtime_ns += 1;
    let mut fs = HashMap::new();
    fs.insert(file_id.to_string(), f);
    assert!(file_needs_index(&file, root, &fs, &store).unwrap());

    // (d) chunk_profile change -> needs index.
    let mut f = matching.clone();
    f.chunk_profile = Some("other-profile".to_string());
    let mut fs = HashMap::new();
    fs.insert(file_id.to_string(), f);
    assert!(file_needs_index(&file, root, &fs, &store).unwrap());

    // (e) chunks == 0 -> up-to-date (no re-index).
    let mut f = matching.clone();
    f.chunks = Some(0);
    let mut fs = HashMap::new();
    fs.insert(file_id.to_string(), f);
    assert!(!file_needs_index(&file, root, &fs, &store).unwrap());
    assert!(text_chunks_up_to_date(&file, root, &fs, &store).unwrap());

    // (f) chunks == 5 but sqlite has no chunks -> needs index.
    let mut fs = HashMap::new();
    fs.insert(file_id.to_string(), matching.clone());
    assert!(file_needs_index(&file, root, &fs, &store).unwrap());
    assert!(!text_chunks_up_to_date(&file, root, &fs, &store).unwrap());

    // (g) chunks == 5 and sqlite has the chunks -> up-to-date.
    store
        .replace_chunks_for_files(
            &[file_id.to_string()],
            &[chunk("sample.py#chunk-0000", file_id)],
        )
        .unwrap();
    assert!(!file_needs_index(&file, root, &fs, &store).unwrap());
    assert!(text_chunks_up_to_date(&file, root, &fs, &store).unwrap());
}

#[test]
fn manifest_legacy_and_load() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("chunk-index-manifest.json");
    // Missing file -> {"files": {}}.
    let m = load_manifest(&path);
    assert!(m.files.is_empty());

    // create=false on an unseen root: Python returns a detached {} and
    // mutates NOTHING — the legacy root/files mirror must stay untouched.
    let mut ro = load_manifest(&path);
    assert!(manifest_files_for_root(&mut ro, dir.path(), false).is_none());
    assert!(ro.root.is_none());
    assert!(ro.files.is_empty());
    assert!(ro.roots.is_empty());

    // Round-trip via manifest_files_for_root (create=true).
    let mut manifest = load_manifest(&path);
    {
        let files = manifest_files_for_root(&mut manifest, dir.path(), true).unwrap();
        files.insert(
            "a.py".to_string(),
            ManifestFileEntry {
                size: 1,
                mtime_ns: 2,
                updated_at: "2025-01-01T00:00:00Z".to_string(),
                chunks: Some(1),
                chunk_profile: Some(active_chunk_profile_version(None)),
            },
        );
    }
    oracle_core::store::manifest::save_manifest(&path, &manifest).unwrap();
    let reloaded = load_manifest(&path);
    assert!(reloaded
        .roots
        .get(&dir.path().to_string_lossy().to_string())
        .unwrap()
        .files
        .contains_key("a.py"));
}

// ── 4. hash_embed parity: covered by the unit test in `store::lance::tests`
//       (`hash_embed_matches_python`) — kept in ONE place on purpose. ────────

// ── 5. LanceStore round-trip ────────────────────────────────────────────

#[tokio::test]
async fn lance_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("chunks.lancedb");
    let store = LanceStore::new(&path);
    store
        .ensure_table(LanceStore::default_schema())
        .await
        .unwrap();

    let texts = [
        "alpha node one",
        "beta node two",
        "gamma node three",
        "delta node four",
        "epsilon node five",
    ];
    let records: Vec<LanceRow> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| LanceRow {
            id: format!("doc-{i}"),
            label: format!("Doc {i}"),
            area: "text".to_string(),
            cluster_semantic: "docs".to_string(),
            vector: hash_embed(t, 1024),
        })
        .collect();

    store.upsert(&records).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 5);

    // upsert with the same ids must MERGE, not append (Python upsert parity).
    store.upsert(&records).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 5, "upsert must dedup by id");

    // search by the first vector returns itself first with score ~1.0.
    let hits = store.search(&records[0].vector, 5).await.unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].id, records[0].id);
    assert!(
        (hits[0].score - 1.0).abs() < 1e-4,
        "self score {}",
        hits[0].score
    );

    // replace_ids updates the row in place (count stays the same).
    let mut updated = records[0].clone();
    updated.label = "Updated Doc 0".to_string();
    store
        .replace_ids(&[records[0].id.clone()], &[updated.clone()])
        .await
        .unwrap();
    assert_eq!(store.count().await.unwrap(), 5);
    let all = store.read_all().await.unwrap();
    let got = all.iter().find(|r| r.id == records[0].id).unwrap();
    assert_eq!(got.label, "Updated Doc 0");

    // similar returns others, excluding self.
    let sim = store.similar(&records[1].id, 5).await.unwrap();
    assert!(sim.iter().all(|h| h.id != records[1].id));
    assert!(!sim.is_empty());

    // delete one id (replace_ids with empty records).
    store
        .replace_ids(&[records[1].id.clone()], &[])
        .await
        .unwrap();
    assert_eq!(store.count().await.unwrap(), 4);
}

#[tokio::test]
async fn lance_json_backend_roundtrip() {
    // `.json` path -> Python's deterministic JSON fallback backend.
    let dir = tempdir().unwrap();
    let path = dir.path().join("vectors.json");
    let store = LanceStore::new(&path);

    let rows: Vec<LanceRow> = (0..3)
        .map(|i| LanceRow {
            id: format!("j-{i}"),
            label: format!("J {i}"),
            area: "a".into(),
            cluster_semantic: "c".into(),
            vector: hash_embed(&format!("json row {i}"), 1024),
        })
        .collect();
    store.upsert(&rows).await.unwrap();
    store.upsert(&rows).await.unwrap(); // merge, not append
    assert_eq!(store.count().await.unwrap(), 3);

    let hits = store.search(&rows[2].vector, 2).await.unwrap();
    assert_eq!(hits[0].id, "j-2");
    assert!((hits[0].score - 1.0).abs() < 1e-4);

    store.replace_ids(&["j-0".to_string()], &[]).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 2);
}

#[tokio::test]
async fn lance_non_default_dims_roundtrip() {
    // file_vectors.lancedb on real installs holds 128-dim vectors: the schema
    // and batches must take dims from the data, never from EMBED_DIMS.
    let dir = tempdir().unwrap();
    let path = dir.path().join("file_vectors.lancedb");
    let store = LanceStore::new(&path);

    let rows: Vec<LanceRow> = (0..4)
        .map(|i| LanceRow {
            id: format!("fv-{i}"),
            label: format!("FV {i}"),
            area: "files".into(),
            cluster_semantic: "cluster".into(),
            vector: hash_embed(&format!("file vector {i}"), 128),
        })
        .collect();
    store.upsert(&rows).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 4);

    let hits = store.search(&rows[1].vector, 2).await.unwrap();
    assert_eq!(hits[0].id, "fv-1");
    assert!((hits[0].score - 1.0).abs() < 1e-4);

    // mixed dims in one batch must fail loudly, not corrupt the table.
    let bad = vec![
        rows[0].clone(),
        LanceRow {
            id: "bad".into(),
            label: "bad".into(),
            area: "a".into(),
            cluster_semantic: "c".into(),
            vector: hash_embed("bad", 64),
        },
    ];
    assert!(store.upsert(&bad).await.is_err());
}

// ── 6. real-store smoke (operator runs manually) ───────────────────────

#[ignore]
#[tokio::test]
async fn real_store_smoke() {
    let base = Path::new("/Users/user/Projects/Devboule/oracle-data");
    let chunks = LanceStore::new(&base.join("chunks.lancedb"));
    let n = chunks.count().await.unwrap();
    assert!(n > 0, "chunks.lancedb had {n} rows");

    let meta = SqliteStore::new(&base.join("metadata.sqlite")).unwrap();
    // node_cards is the LEGACY store (populated only by the build-cards CLI) and
    // is legitimately empty on real installs; the live pipeline fills file_chunks.
    let chunks_in_sqlite = meta.chunk_count().unwrap();
    assert!(
        chunks_in_sqlite > 0,
        "metadata.sqlite had {chunks_in_sqlite} file_chunks"
    );
    println!(
        "real store: {n} lance rows, {chunks_in_sqlite} sqlite chunks, {} node cards",
        meta.count().unwrap()
    );
}
