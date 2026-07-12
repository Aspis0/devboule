//! int8 vs fp32 RETRIEVAL-QUALITY comparison (operator-run, `--ignored`).
//!
//! The owner shipped int8 (smaller/faster) but worries about QUALITY. Raw vector
//! drift (int8 vs fp32 cosine 0.70-0.91) is scary on paper, but what actually
//! matters is whether the RIGHT chunks still rank on top. This test measures that
//! directly: it indexes the SAME corpus twice — once fp32, once int8 — and runs a
//! battery of semantically-worded queries (each avoiding its target's literal
//! tokens, so DENSE retrieval drives ranking), then prints top-1 accuracy,
//! recall@3, and the score margin (correct vs best decoy) side by side.
//!
//! Both variants are forced onto the CPU EP so the ONLY difference measured is
//! quantization (fp32 vs int8), never the execution provider.
//!
//! Requires the local model bundle at `models/qwen3-onnx/` holding BOTH
//! `onnx/model.onnx` (+ `.onnx_data`) and `onnx/model_int8.onnx`. Run with:
//!   cargo test --test int8_quality_test -- --ignored --nocapture
//!
//! Reading the result: if int8's top-1 / recall@3 match fp32 and the margins are
//! comparable, int8 quality is fine — ship it. If int8 drops queries fp32 got, or
//! its margins collapse toward zero, quality degraded and fp32 is the better call
//! (reverting is a 6-flag flip + re-index).

use oracle_core::embed::{BackendChoice, CancelFlag, EmbedderPool};
use oracle_core::ingest::indexer::{self, IndexerConfig};
use oracle_core::query::engine::{QueryEmbedder, QueryEngine};
use oracle_core::store::lance::LanceStore;
use oracle_core::store::sqlite::SqliteStore;
use std::path::{Path, PathBuf};

fn model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/qwen3-onnx")
}

struct PoolQuery<'a> {
    pool: &'a EmbedderPool,
    cancel: &'a CancelFlag,
}

impl QueryEmbedder for PoolQuery<'_> {
    fn embed_query(&self, text: &str, _dims: usize) -> anyhow::Result<Vec<f32>> {
        let vecs = self.pool.embed(&[text.to_string()], 8, self.cancel)?;
        vecs.into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("embedder returned no vector"))
    }
}

/// 12 short docs across 12 distinct semantic domains.
const CORPUS: &[(&str, &str)] = &[
    ("payments.md", "Funds are drawn from a subscriber's stored card when each cycle closes; the statement lists every line and the sum due."),
    ("moon.md", "Our natural satellite circles the planet roughly every twenty-seven days, bound in place by gravitation."),
    ("sourdough.md", "A wild yeast culture is refreshed with flour and water daily until it bubbles and rises, fit for the oven."),
    ("tcp.md", "Two machines agree on a connection with a three-step exchange of synchronize and acknowledge signals before any data flows."),
    ("roses.md", "Cut back the woody stems of the shrub in early spring so fresh canes and blooms emerge later in the summer."),
    ("insulin.md", "The pancreas releases a hormone that lets cells take in sugar from the blood; without it, those levels climb dangerously."),
    ("contract.md", "When one party fails to honor a signed agreement, the other may seek damages or demand the promised performance in court."),
    ("scales.md", "A major key stacks seven tones at fixed intervals; chords are built by sounding several of those tones together."),
    ("hurricane.md", "Over warm tropical seas, rising moist air spins into a vast rotating storm with a calm eye and violent bands of wind."),
    ("rsa.md", "A message is locked with a widely shared key and can be opened only with a secret counterpart, so anyone may write in confidence."),
    ("aperture.md", "Widening the opening inside the lens admits more light and softens the background, while a narrow one keeps the whole frame crisp."),
    ("aqueduct.md", "The ancient builders carried flowing water across valleys on tall arched channels of stone, sloping gently onward for miles."),
];

/// (query text worded to avoid the target's literal tokens, expected file needle).
const QUERIES: &[(&str, &str)] = &[
    ("how recurring charges are collected from customers each month", "payments"),
    ("orbital period of the lunar body around earth", "moon"),
    ("keeping a bread leaven alive before baking", "sourdough"),
    ("reliable handshake establishing a network link between hosts", "tcp"),
    ("pruning garden flower bushes for healthier growth", "roses"),
    ("regulating glucose in patients living with diabetes", "insulin"),
    ("legal remedies for breaking a business deal", "contract"),
    ("harmony and melody structure in a written song", "scales"),
    ("formation of a cyclone above heated ocean water", "hurricane"),
    ("asymmetric encryption using public and private keys", "rsa"),
    ("controlling exposure and depth of field on a camera", "aperture"),
    ("how the romans transported drinking supply over long distances", "aqueduct"),
];

#[derive(Clone)]
struct QResult {
    query: String,
    expected: String,
    /// 1-based rank of the first chunk from the expected file, or None if absent.
    rank: Option<usize>,
    /// Best score among chunks from the expected file (f64::MIN if none).
    expected_score: f64,
    /// Margin = expected best score − best decoy score (positive ⇒ expected wins).
    margin: f64,
}

async fn run_variant(corpus_root: &Path, data: &Path, model: PathBuf, int8: bool) -> Vec<QResult> {
    std::fs::create_dir_all(data).unwrap();
    let sqlite = SqliteStore::new(&data.join("metadata.sqlite")).unwrap();
    let chunk_vectors = LanceStore::new(&data.join("chunks.lancedb"));
    let manifest = data.join("chunk-index-manifest.json");

    let pool = EmbedderPool::new(BackendChoice::Ort {
        model_dir: model,
        int8,
    });
    let cancel = CancelFlag::new();
    let cfg = IndexerConfig {
        min_free_gb: 0.0,
        max_gpu_temp_c: None,
        ..Default::default()
    };
    indexer::index_file_chunks(
        corpus_root,
        &sqlite,
        &chunk_vectors,
        &manifest,
        &pool,
        &cancel,
        &cfg,
        None,
    )
    .await
    .unwrap();

    let engine = QueryEngine::new(
        sqlite,
        LanceStore::new(&data.join("node_vectors.lancedb")),
        Some(chunk_vectors),
        Some(LanceStore::new(&data.join("file_vectors.lancedb"))),
    );
    let embedder = PoolQuery { pool: &pool, cancel: &cancel };

    let mut results = Vec::new();
    for (query, expected) in QUERIES {
        let rows = engine
            .context(query, 10, &embedder, None, false, None, None, None, None, None)
            .await
            .unwrap();
        let rank = rows
            .iter()
            .position(|c| c.file_source.contains(expected))
            .map(|i| i + 1);
        let expected_score = rows
            .iter()
            .filter(|c| c.file_source.contains(expected))
            .map(|c| c.score)
            .fold(f64::MIN, f64::max);
        let decoy_score = rows
            .iter()
            .filter(|c| !c.file_source.contains(expected))
            .map(|c| c.score)
            .fold(f64::MIN, f64::max);
        let margin = if expected_score == f64::MIN {
            f64::MIN
        } else {
            expected_score - decoy_score
        };
        results.push(QResult {
            query: (*query).to_string(),
            expected: (*expected).to_string(),
            rank,
            expected_score,
            margin,
        });
    }
    results
}

fn summarize(label: &str, r: &[QResult]) -> (usize, usize) {
    let top1 = r.iter().filter(|q| q.rank == Some(1)).count();
    let recall3 = r.iter().filter(|q| matches!(q.rank, Some(n) if n <= 3)).count();
    println!("\n── {label} ──  top1={top1}/{n}  recall@3={recall3}/{n}", n = r.len());
    for q in r {
        let rank = q.rank.map(|n| n.to_string()).unwrap_or_else(|| "MISS".into());
        let score = if q.expected_score == f64::MIN { "n/a".into() } else { format!("{:.4}", q.expected_score) };
        let margin = if q.margin == f64::MIN { "n/a".into() } else { format!("{:+.4}", q.margin) };
        println!("   [{rank:>4}] margin {margin:>9}  score {score:>7}  <- {} ({})", q.expected, q.query);
    }
    (top1, recall3)
}

#[tokio::test]
#[ignore]
async fn int8_vs_fp32_retrieval_quality() {
    // Isolate the variable: force CPU for BOTH variants so the only difference is
    // quantization (fp32 vs int8), not the execution provider.
    std::env::set_var("ORACLE_RS_EP", "cpu");

    let model = model_dir();
    if !model.join("onnx/model_int8.onnx").exists() || !model.join("onnx/model.onnx").exists() {
        eprintln!(
            "skipping: need BOTH fp32 (onnx/model.onnx) and int8 (onnx/model_int8.onnx) at {}",
            model.display()
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&corpus).unwrap();
    for (name, body) in CORPUS {
        std::fs::write(corpus.join(name), body).unwrap();
    }

    // Run fp32 fully, then int8 fully (sequential — the ONNX variant is selected
    // via a process-global env var at model-load time, so never interleave loads).
    let fp32 = run_variant(&corpus, &dir.path().join("data-fp32"), model.clone(), false).await;
    let int8 = run_variant(&corpus, &dir.path().join("data-int8"), model.clone(), true).await;

    let (fp32_top1, fp32_r3) = summarize("fp32 (baseline)", &fp32);
    let (int8_top1, int8_r3) = summarize("int8 (shipped)", &int8);

    // Per-query margin delta (how much int8 erodes the correct-vs-decoy gap).
    println!("\n── per-query int8−fp32 margin delta (negative = int8 worse) ──");
    for (a, b) in fp32.iter().zip(int8.iter()) {
        let d = if a.margin == f64::MIN || b.margin == f64::MIN {
            "n/a".to_string()
        } else {
            format!("{:+.4}", b.margin - a.margin)
        };
        let flip = match (a.rank == Some(1), b.rank == Some(1)) {
            (true, false) => "  <-- int8 LOST top-1",
            (false, true) => "  <-- int8 GAINED top-1",
            _ => "",
        };
        println!("   {:>9}   {}{}", d, a.expected, flip);
    }

    println!(
        "\n==== SUMMARY ====\n  fp32: top1={fp32_top1}/{n}  recall@3={fp32_r3}/{n}\n  int8: top1={int8_top1}/{n}  recall@3={int8_r3}/{n}",
        n = QUERIES.len()
    );
    println!(
        "  int8 vs fp32: top1 {:+}, recall@3 {:+}",
        int8_top1 as i64 - fp32_top1 as i64,
        int8_r3 as i64 - fp32_r3 as i64
    );

    // Soft guard: fail LOUD only if int8 is badly degraded vs fp32 (more than a
    // 2-query top-1 regression). This is a MEASUREMENT test first — read the table
    // above — but a big drop should not pass silently.
    assert!(
        int8_top1 + 2 >= fp32_top1,
        "int8 top-1 regressed hard vs fp32 ({int8_top1} vs {fp32_top1}) — quality concern is real; consider reverting to fp32"
    );
}
