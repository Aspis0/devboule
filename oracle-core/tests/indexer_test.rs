//! Integration tests for `ingest::indexer` — the chunk indexing pipeline.
//!
//! All tests use a tempdir world with a small corpus copied from
//! `golden/corpus/` and a `FakeEmbedder` that produces deterministic vectors
//! without loading any model.

use oracle_core::config::EMBED_DIMS;
use oracle_core::embed::CancelFlag;
use oracle_core::ingest::indexer::{self, IndexStatus, IndexerConfig, TextEmbedder};
use oracle_core::store::lance::LanceStore;
use oracle_core::store::manifest::{load_manifest, manifest_files_for_root};
use oracle_core::store::sqlite::SqliteStore;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// Fake embedder
// ═══════════════════════════════════════════════════════════════════════════

/// Deterministic fake embedder for hermetic tests.
struct FakeEmbedder {
    call_count: AtomicUsize,
    /// When `Some(n)`, cancel after the n-th embed call.
    cancel_after: Option<usize>,
}

impl FakeEmbedder {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            cancel_after: None,
        }
    }

    fn with_cancel_after(n: usize) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            cancel_after: Some(n),
        }
    }

    fn calls(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

impl TextEmbedder for FakeEmbedder {
    fn embed(
        &self,
        texts: &[String],
        _batch_size: usize,
        cancel: &CancelFlag,
    ) -> anyhow::Result<Vec<Vec<f32>>> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if let Some(after) = self.cancel_after {
            if count >= after {
                cancel.cancel();
                // Still return vectors for the texts processed before cancel
            }
        }
        // Deterministic pseudo-vectors: each text gets a unique but stable vector
        Ok(texts
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let mut vec = vec![0.0f32; EMBED_DIMS];
                // Seed from text bytes + call count + index for uniqueness
                for (j, byte) in text.bytes().enumerate() {
                    let idx = (j + count * 7 + i * 13) % EMBED_DIMS;
                    vec[idx] += byte as f32;
                }
                // Normalize
                let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for v in &mut vec {
                        *v /= norm;
                    }
                }
                vec
            })
            .collect())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test world setup
// ═══════════════════════════════════════════════════════════════════════════

struct TestWorld {
    _dir: tempfile::TempDir,
    root: PathBuf,
    sqlite_path: PathBuf,
    vector_path: PathBuf,
    manifest_path: PathBuf,
}

impl TestWorld {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let oracle_data = root.join("oracle-data");
        std::fs::create_dir_all(&oracle_data).unwrap();

        let sqlite_path = oracle_data.join("metadata.sqlite");
        let vector_path = oracle_data.join("chunks.lancedb");
        let manifest_path = oracle_data.join("chunk-index-manifest.json");

        // Copy a small corpus from golden/corpus
        let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("golden/corpus");
        let files_to_copy = &[
            ("src/app.py", "fn main():\n    print('hello world')\n"),
            ("src/lib.rs", "pub fn helper() -> i32 { 42 }\n"),
            (
                "docs/architecture.md",
                "# Architecture\n\nThis is the plan.\n",
            ),
            ("data/config.json", "{\"key\": \"value\", \"count\": 42}\n"),
        ];

        for (rel, fallback_content) in files_to_copy {
            let src = corpus.join(rel);
            let dst = root.join(rel);
            std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
            if src.exists() {
                std::fs::copy(&src, &dst).unwrap();
            } else {
                std::fs::write(&dst, fallback_content).unwrap();
            }
        }

        TestWorld {
            _dir: dir,
            root,
            sqlite_path,
            vector_path,
            manifest_path,
        }
    }

    fn sqlite(&self) -> SqliteStore {
        SqliteStore::new(&self.sqlite_path).unwrap()
    }

    fn vectors(&self) -> LanceStore {
        LanceStore::new(&self.vector_path)
    }

    fn config(&self) -> IndexerConfig {
        IndexerConfig {
            min_free_gb: 0.0,     // disable RAM guard in tests
            max_gpu_temp_c: None, // disable GPU guard in tests
            // FakeEmbedder loads no model: keep file-batch → one embed call so
            // cancel/max_batches tests stay deterministic (production defaults
            // intentionally split by attention cost).
            attention_budget: usize::MAX / 4,
            batch_chars: 10_000_000,
            ..Default::default()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 1: Fresh index — all files indexed, stores consistent, status=complete
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_fresh_index_complete() {
    let world = TestWorld::new();
    let sqlite = world.sqlite();
    let vectors = world.vectors();
    let embedder = FakeEmbedder::new();
    let cancel = CancelFlag::new();

    let result = indexer::index_file_chunks(
        &world.root,
        &sqlite,
        &vectors,
        &world.manifest_path,
        &embedder,
        &cancel,
        &world.config(),
        None,
    )
    .await
    .unwrap();

    // Status should be complete
    assert_eq!(result.status, IndexStatus::Complete);

    // All 4 files should be indexed
    assert_eq!(result.scanned, 4, "should scan all 4 corpus files");
    assert_eq!(result.processed, 4, "should process all 4 files");
    assert!(result.chunks > 0, "should produce chunks");

    // SQLite should have chunks for all files
    let chunk_file_count = sqlite.chunk_file_count().unwrap();
    assert_eq!(chunk_file_count, 4, "sqlite should have 4 chunk files");

    let chunk_count = sqlite.chunk_count().unwrap();
    assert!(chunk_count > 0, "sqlite should have chunks");
    assert_eq!(chunk_count, result.chunks, "chunk count should match");

    // Lance vectors should match chunk count
    let vector_count = vectors.count().await.unwrap();
    assert_eq!(
        vector_count, chunk_count,
        "vector count should equal chunk count"
    );

    // Manifest should have entries for all files
    let mut manifest = load_manifest(&world.manifest_path);
    let manifest_files = manifest_files_for_root(&mut manifest, &world.root, false)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        manifest_files.len(),
        4,
        "manifest should have 4 file entries"
    );

    // Every manifest entry should have chunk metadata
    for (file_id, entry) in &manifest_files {
        assert!(
            entry.chunks.is_some() && entry.chunks.unwrap() > 0,
            "manifest entry for {} should have chunks > 0",
            file_id
        );
        assert!(
            entry.chunk_profile.is_some(),
            "manifest entry for {} should have chunk_profile",
            file_id
        );
    }

    // Consistency: manifest total chunks == sqlite chunk count
    let manifest_total: u64 = manifest_files.values().filter_map(|e| e.chunks).sum();
    assert_eq!(
        manifest_total, chunk_count as u64,
        "manifest total chunks should equal sqlite chunks"
    );

    // Embedder should have been called at least once
    assert!(embedder.calls() > 0, "embedder should have been called");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 2: Incremental — touch ONE file → only that file re-indexed
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_incremental_reindex() {
    let world = TestWorld::new();
    let sqlite = world.sqlite();
    let vectors = world.vectors();
    let embedder = FakeEmbedder::new();
    let cancel = CancelFlag::new();

    // First run: full index
    let r1 = indexer::index_file_chunks(
        &world.root,
        &sqlite,
        &vectors,
        &world.manifest_path,
        &embedder,
        &cancel,
        &world.config(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(r1.status, IndexStatus::Complete);
    let first_run_calls = embedder.calls();

    // Touch one file (change content)
    let app_py = world.root.join("src/app.py");
    std::fs::write(&app_py, "fn main():\n    print('modified content')\n").unwrap();

    // Second run: incremental
    let r2 = indexer::index_file_chunks(
        &world.root,
        &sqlite,
        &vectors,
        &world.manifest_path,
        &embedder,
        &cancel,
        &world.config(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(r2.status, IndexStatus::Complete);

    // Only 1 file should have been re-indexed
    assert_eq!(
        r2.processed, 1,
        "only the touched file should be re-indexed"
    );

    // Embedder should have been called at least once more (for the 1 file)
    assert!(
        embedder.calls() > first_run_calls,
        "embedder should be called again for the touched file"
    );

    // All 4 files should still be in sqlite
    assert_eq!(sqlite.chunk_file_count().unwrap(), 4);

    // Manifest should still have 4 entries
    let mut manifest = load_manifest(&world.manifest_path);
    let mf = manifest_files_for_root(&mut manifest, &world.root, false)
        .cloned()
        .unwrap_or_default();
    assert_eq!(mf.len(), 4);
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 3: Prune — delete a file → removes chunks/vectors/manifest + orphan
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_prune_removes_stale() {
    let world = TestWorld::new();
    let sqlite = world.sqlite();
    let vectors = world.vectors();
    let embedder = FakeEmbedder::new();
    let cancel = CancelFlag::new();

    // Full index
    indexer::index_file_chunks(
        &world.root,
        &sqlite,
        &vectors,
        &world.manifest_path,
        &embedder,
        &cancel,
        &world.config(),
        None,
    )
    .await
    .unwrap();

    let chunks_before = sqlite.chunk_count().unwrap();
    let vectors_before = vectors.count().await.unwrap();
    assert!(chunks_before > 0);
    assert_eq!(chunks_before, vectors_before);

    // Delete a file
    std::fs::remove_file(world.root.join("data/config.json")).unwrap();

    // Run prune
    let pr = indexer::prune_excluded_chunks(
        &world.root,
        &sqlite,
        &vectors,
        &world.manifest_path,
        None, // no node vector store
        None,
    )
    .await
    .unwrap();

    assert_eq!(pr.status, "complete");
    assert!(
        pr.removed_files >= 1,
        "should remove at least 1 file's chunks"
    );
    assert!(pr.removed_vectors >= 1, "should remove at least 1 vector");

    // Manifest should have 3 entries now
    let mut manifest = load_manifest(&world.manifest_path);
    let mf = manifest_files_for_root(&mut manifest, &world.root, false)
        .cloned()
        .unwrap_or_default();
    assert_eq!(mf.len(), 3, "manifest should have 3 entries after prune");

    // Now inject an orphan vector (id not in sqlite)
    let orphan_id = "orphan-fake-vector-id".to_string();
    let orphan_row = oracle_core::store::lance::LanceRow {
        id: orphan_id.clone(),
        label: "orphan".to_string(),
        area: "FileChunk".to_string(),
        cluster_semantic: "text".to_string(),
        vector: vec![0.1; EMBED_DIMS],
    };
    vectors.replace_ids(&[], &[orphan_row]).await.unwrap();
    assert_eq!(
        vectors.count().await.unwrap(),
        pr.vector_records + 1,
        "orphan vector should be present before second prune"
    );

    // Second prune should remove the orphan
    let pr2 = indexer::prune_excluded_chunks(
        &world.root,
        &sqlite,
        &vectors,
        &world.manifest_path,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        pr2.removed_orphan_vectors >= 1,
        "should remove orphan vectors"
    );

    // Orphan should be gone
    let all_rows = vectors.read_all().await.unwrap();
    assert!(
        !all_rows.iter().any(|r| r.id == orphan_id),
        "orphan vector should be removed"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 4: max_batches=1 → paused_batch_limit with partial progress
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_max_batches_pause() {
    let world = TestWorld::new();
    let sqlite = world.sqlite();
    let vectors = world.vectors();
    let embedder = FakeEmbedder::new();
    let cancel = CancelFlag::new();

    let mut config = world.config();
    config.max_batches = Some(1);
    config.batch_files = 1; // process 1 file per batch → 1 total with max_batches=1

    let result = indexer::index_file_chunks(
        &world.root,
        &sqlite,
        &vectors,
        &world.manifest_path,
        &embedder,
        &cancel,
        &config,
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        result.status,
        IndexStatus::PausedBatchLimit,
        "should pause after 1 batch"
    );
    assert_eq!(result.processed, 1, "should have processed exactly 1 file");
    assert!(
        result.pending.is_some() && result.pending.unwrap() > 0,
        "should have pending files remaining"
    );
    assert!(
        result.pending.unwrap() + result.processed == 4,
        "pending + processed should equal total files"
    );

    // Some chunks should be in sqlite
    assert!(sqlite.chunk_count().unwrap() > 0);
    // Some vectors should be in lance
    assert!(vectors.count().await.unwrap() > 0);
    // Manifest should have 1 entry
    let mut manifest = load_manifest(&world.manifest_path);
    let mf = manifest_files_for_root(&mut manifest, &world.root, false)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        mf.len(),
        1,
        "manifest should have 1 entry after max_batches=1"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 5: Cancel mid-run → clean partial state, stores agree
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_cancel_mid_run() {
    let world = TestWorld::new();
    let sqlite = world.sqlite();
    let vectors = world.vectors();
    // Cancel after the 1st embed call (processes first file batch, then
    // cancels on the second batch's embed)
    let embedder = FakeEmbedder::with_cancel_after(1);
    let cancel = CancelFlag::new();

    let mut config = world.config();
    config.batch_files = 2; // 2 files per batch

    let result = indexer::index_file_chunks(
        &world.root,
        &sqlite,
        &vectors,
        &world.manifest_path,
        &embedder,
        &cancel,
        &config,
        None,
    )
    .await
    .unwrap();

    // Should have processed at least 2 files (first batch completed before cancel)
    assert!(
        result.processed >= 2,
        "should have processed at least the first batch (2 files), got {}",
        result.processed
    );
    assert!(
        result.processed < 4,
        "should NOT have processed all 4 files due to cancel"
    );

    // Consistency: sqlite chunk count == vector count for committed files
    let chunk_count = sqlite.chunk_count().unwrap();
    let vector_count = vectors.count().await.unwrap();
    assert_eq!(
        chunk_count, vector_count,
        "sqlite chunks and lance vectors should be consistent after cancel"
    );

    // Manifest entries should equal processed files
    let mut manifest = load_manifest(&world.manifest_path);
    let mf = manifest_files_for_root(&mut manifest, &world.root, false)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        mf.len(),
        result.processed,
        "manifest entries should match processed file count"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 6: sync_text_chunks fills sqlite but writes NO vectors
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_sync_text_chunks_no_vectors() {
    let world = TestWorld::new();
    let sqlite = world.sqlite();
    let _vectors = world.vectors();

    // Verify lance is empty before
    // (vectors.count() is async, but we haven't written anything, so it's 0)
    // We'll check after sync that vectors are still 0.

    let result =
        indexer::sync_text_chunks(&world.root, &sqlite, &world.manifest_path, 100, false, None)
            .unwrap();

    assert_eq!(result.status, "complete");
    assert_eq!(result.files, 4, "should sync all 4 files");
    assert!(result.chunks > 0, "should produce chunks");
    assert_eq!(result.skipped, 0, "nothing to skip on first run");

    // SQLite should have chunks
    let chunk_count = sqlite.chunk_count().unwrap();
    assert!(chunk_count > 0, "sqlite should have chunks after sync");
    assert_eq!(chunk_count, result.chunks);

    // Lance should have NO vectors (sync_text_chunks doesn't touch it)
    // Note: we can't easily call .await here in a sync test, so we check
    // by verifying that vector_records is not in the manifest path.
    // Instead, verify via the result: it has no vector info at all.
    // The key invariant: sync_text_chunks never calls LanceStore.
    // We can verify this by checking that the Lance DB file was not created
    // (or is empty) if it didn't exist before.
    let lance_dir = &world.vector_path;
    // LanceStore may create the dir on open; check if it has data by
    // trying to open it (it returns 0 if empty/missing)
    // We'll verify this with a fresh LanceStore
    let fresh_vectors = LanceStore::new(lance_dir);
    // count() is async, but we can use block_on for this simple check
    let count = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(fresh_vectors.count())
        .unwrap();
    assert_eq!(
        count, 0,
        "lance should have NO vectors after sync_text_chunks"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 7: chunk_index_status counts correct before/after
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_chunk_index_status_before_after() {
    let world = TestWorld::new();
    let sqlite = world.sqlite();
    let vectors = world.vectors();

    // Status before any indexing
    let status_before =
        indexer::chunk_index_status(&world.root, &sqlite, &vectors, &world.manifest_path)
            .await
            .unwrap();

    assert_eq!(status_before.expected_files, 4);
    assert_eq!(status_before.indexed_files, 0);
    assert_eq!(status_before.pending_files, 4);
    assert_eq!(status_before.sqlite_chunk_files, 0);
    assert_eq!(status_before.sqlite_chunks, 0);
    assert_eq!(status_before.vector_records, 0);
    assert!(status_before.first_pending.len() == 4);

    // Full index
    let embedder = FakeEmbedder::new();
    let cancel = CancelFlag::new();
    indexer::index_file_chunks(
        &world.root,
        &sqlite,
        &vectors,
        &world.manifest_path,
        &embedder,
        &cancel,
        &world.config(),
        None,
    )
    .await
    .unwrap();

    // Status after indexing
    let status_after =
        indexer::chunk_index_status(&world.root, &sqlite, &vectors, &world.manifest_path)
            .await
            .unwrap();

    assert_eq!(status_after.expected_files, 4);
    assert_eq!(status_after.indexed_files, 4);
    assert_eq!(status_after.pending_files, 0);
    assert_eq!(status_after.stale_files, 0);
    assert_eq!(status_after.sqlite_chunk_files, 4);
    assert!(status_after.sqlite_chunks > 0);
    assert_eq!(status_after.vector_records, status_after.sqlite_chunks);
    assert!(status_after.first_pending.is_empty());
    assert!(status_after.first_stale.is_empty());

    // Touch a file and check stale count
    std::fs::write(
        world.root.join("src/lib.rs"),
        "pub fn helper() -> i32 { 99 }\n",
    )
    .unwrap();

    let status_stale =
        indexer::chunk_index_status(&world.root, &sqlite, &vectors, &world.manifest_path)
            .await
            .unwrap();

    assert_eq!(status_stale.stale_files, 1, "1 file should be stale");
    assert_eq!(
        status_stale.pending_files, 0,
        "no pending files (all indexed)"
    );
}
