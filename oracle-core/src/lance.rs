use anyhow::{Context, Result};
use arrow_array::{Array, Float32Array, RecordBatch, StringArray};
use crate::embedder::{load_model, resolve_device, DtypeArg, DeviceArg};
use crate::BackendArg;
use crate::onnx_embedder::{EpArg, OnnxEmbedder};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::DistanceType;
use serde::Serialize;
use std::path::PathBuf;

/// One result row. `id`/`label` are read defensively and emitted as `null`
/// when the column is missing or the row is null.
#[derive(Debug, Serialize)]
struct QueryRow {
    id: Option<String>,
    label: Option<String>,
    score: Option<f64>,
}

/// `query` subcommand: embed a text, run a LanceDB cosine nearest-neighbour
/// search over the `nodes` table, print top-N rows as JSON lines.
pub async fn cmd_query(
    db: PathBuf,
    query: String,
    limit: usize,
    backend: BackendArg,
    device_arg: DeviceArg,
    dtype_arg: DtypeArg,
    model_dir: PathBuf,
    ep: EpArg,
    _batch_size: usize,
) -> Result<()> {
    let query_vec = if matches!(backend, BackendArg::Onnx) {
        let (mut embedder, load_ms) = OnnxEmbedder::load(model_dir.as_path(), ep)?;
        eprintln!("model load: {} ms", load_ms);
        let vectors = embedder
            .embed_batched(&[query.clone()], 1)
            .context("embedding query failed")?;
        vectors
            .into_iter()
            .next()
            .context("query embedding produced no vector")?
    } else {
        let device = resolve_device(device_arg)?;
        let dtype = dtype_arg.to_dtype();

        let loaded = load_model(&device, dtype)?;
        eprintln!("model load: {} ms", loaded.load_ms);

        let q = loaded
            .model
            .embed(&[query.clone()])
            .with_context(|| "embedding query failed")?;
        q.into_iter()
            .next()
            .context("query embedding produced no vector")?
    };

    let conn = lancedb::connect(&db.to_string_lossy())
        .execute()
        .await
        .with_context(|| format!("connecting to LanceDB at {}", db.display()))?;
    let table = conn
        .open_table("nodes")
        .execute()
        .await
        .with_context(|| format!("opening table 'nodes' in {}", db.display()))?;

    let stream = table
        .query()
        .nearest_to(query_vec)?
        .distance_type(DistanceType::Cosine)
        .limit(limit)
        .execute()
        .await
        .context("executing nearest-neighbour query")?;

    let batches: Vec<RecordBatch> = stream
        .try_collect()
        .await
        .context("collecting result batches")?;

    for batch in &batches {
        let n = batch.num_rows();

        let ids = batch
            .column_by_name("id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let labels = batch
            .column_by_name("label")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let dist = batch
            .column_by_name("_distance")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

        for row in 0..n {
            let id = ids.and_then(|a| {
                if a.is_null(row) {
                    None
                } else {
                    Some(a.value(row).to_string())
                }
            });
            let label = labels.and_then(|a| {
                if a.is_null(row) {
                    None
                } else {
                    Some(a.value(row).to_string())
                }
            });
            let score = dist.and_then(|a| {
                if a.is_null(row) {
                    None
                } else {
                    Some(1.0 - a.value(row) as f64)
                }
            });

            let out = QueryRow { id, label, score };
            println!("{}", serde_json::to_string(&out)?);
        }
    }

    Ok(())
}
