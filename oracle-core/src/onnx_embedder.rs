use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::ValueEnum;
use ort::session::{Session, builder::GraphOptimizationLevel};
use tokenizers::{
    PaddingDirection, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams,
    TruncationStrategy,
};

/// Public model identifier used in JSON output for the ONNX backend.
pub const ONNX_MODEL_ID: &str = "Qwen3-Embedding-0.6B-ONNX-int8";

/// CLI-facing execution-provider selector for the ONNX backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EpArg {
    Cpu,
    Coreml,
}

/// ONNX (`ort`) embedding backend: a compiled session plus a tokenizer.
///
/// The session is built once per run and reused across batches.
/// Geometry of Qwen3-Embedding-0.6B (config.json: num_hidden_layers=28,
/// num_key_value_heads=8, head_dim=128) — used to feed empty KV caches.
const KV_LAYERS: usize = 28;
const KV_HEADS: usize = 8;
const KV_HEAD_DIM: usize = 128;

pub struct OnnxEmbedder {
    session: Session,
    tokenizer: Tokenizer,
}

impl OnnxEmbedder {
    /// Load the graph + tokenizer from `model_dir` and optionally select an EP.
    ///
    /// Returns the embedder plus the wall-clock load time in milliseconds.
    pub fn load(model_dir: &Path, ep: EpArg) -> Result<(Self, u128)> {
        let start = Instant::now();

        let variant = std::env::var("ORACLE_RS_ONNX_VARIANT").unwrap_or_else(|_| "int8".into());
        let model_file = if variant == "fp32" { "model.onnx".to_string() } else { format!("model_{variant}.onnx") };
        let model_path = model_dir.join("onnx").join(model_file);
        let tokenizer_path = model_dir.join("tokenizer.json");

        let session_builder = Session::builder()
            .context("failed to create ONNX session builder")?;
        let mut builder = session_builder
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("failed to set ONNX optimization level: {e}"))?
            .with_intra_threads(
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4),
            )
            .map_err(|e| anyhow::anyhow!("failed to set ONNX intra-op threads: {e}"))?;

        #[cfg(target_os = "macos")]
        {
            use ort::ep;
            if matches!(ep, EpArg::Coreml) {
                builder = builder
                    .with_execution_providers([ep::CoreML::default()
                        .with_model_format(ep::coreml::ModelFormat::MLProgram)
                        .build()])
                    .map_err(|e| anyhow::anyhow!("failed to register CoreML execution provider: {e}"))?;
            }
        }
        #[cfg(not(target_os = "macos"))]
        if matches!(ep, EpArg::Coreml) {
            anyhow::bail!("--ep coreml is only supported on macOS builds");
        }

        let session = builder
            .commit_from_file(&model_path)
            .with_context(|| {
                format!("failed to build ONNX session from {}", model_path.display())
            })?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            anyhow::anyhow!("failed to load tokenizer from {}: {e}", tokenizer_path.display())
        })?;

        let embedder = OnnxEmbedder { session, tokenizer };
        Ok((embedder, start.elapsed().as_millis()))
    }

    /// Embed `texts` in chunks of `batch_size`.
    ///
    /// Pooling is last-real-token (right padding), matching the candle path,
    /// and each vector is L2-normalized.
    pub fn embed_batched(&mut self, texts: &[String], batch_size: usize) -> Result<Vec<Vec<f32>>> {
        let batch_size = batch_size.max(1);

        // Right-pad to the longest sequence in each batch; truncate to 8192.
        self.tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            direction: PaddingDirection::Right,
            ..Default::default()
        }));
        self.tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: 8192,
                strategy: TruncationStrategy::LongestFirst,
                stride: 0,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("failed to configure tokenizer truncation: {e}"))?;

        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(batch_size) {
            let batch_texts: Vec<String> = chunk.to_vec();
            let encodings = self
                .tokenizer
                .encode_batch(batch_texts, true)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let seq_len = encodings
                .iter()
                .map(|e| e.get_ids().len())
                .max()
                .unwrap_or(0)
                .max(1);
            let batch = encodings.len();

            let mut ids_vec: Vec<i64> = Vec::with_capacity(batch * seq_len);
            let mut mask_vec: Vec<i64> = Vec::with_capacity(batch * seq_len);
            let mut pos_vec: Vec<i64> = Vec::with_capacity(batch * seq_len);

            for enc in &encodings {
                let ids = enc.get_ids();
                let attn = enc.get_attention_mask();
                for j in 0..seq_len {
                    let id = if j < ids.len() { ids[j] as i64 } else { 0 };
                    let m = if j < attn.len() { attn[j] as i64 } else { 0 };
                    ids_vec.push(id);
                    mask_vec.push(m);
                    pos_vec.push(j as i64);
                }
            }

            let mut run_inputs = ort::inputs![
                "input_ids" => ort::value::Tensor::from_array(([batch, seq_len], ids_vec.into_boxed_slice()))?,
                "attention_mask" => ort::value::Tensor::from_array(([batch, seq_len], mask_vec.into_boxed_slice()))?,
                "position_ids" => ort::value::Tensor::from_array(([batch, seq_len], pos_vec.into_boxed_slice()))?,
            ];
            // This export was traced with a KV cache: the graph declares
            // past_key_values.<layer>.{key,value} as REQUIRED inputs. Feed
            // zero-length caches ([batch, kv_heads, 0, head_dim]) so the
            // model runs as a plain encoder.
            for layer in 0..KV_LAYERS {
                for kind in ["key", "value"] {
                    let empty_kv = ort::value::Tensor::<f32>::new(
                        self.session.allocator(),
                        [batch as i64, KV_HEADS as i64, 0, KV_HEAD_DIM as i64],
                    )
                    .context("failed to allocate empty KV-cache tensor")?;
                    run_inputs.push((
                        format!("past_key_values.{layer}.{kind}").into(),
                        empty_kv.into(),
                    ));
                }
            }
            let outputs = self
                .session
                .run(run_inputs)
                .context("ONNX session run failed")?;

            let (shape, data) = outputs["last_hidden_state"]
                .try_extract_tensor::<f32>()
                .context("failed to extract last_hidden_state tensor")?;
            let seq = shape[1] as usize;
            let hidden = shape[2] as usize;

            for row in 0..batch {
                let mask_sum: i64 = encodings[row]
                    .get_attention_mask()
                    .iter()
                    .map(|&x| x as i64)
                    .sum();
                // With right padding the last real token is just before the pad run.
                let real_last = (mask_sum - 1) as usize;
                let base = (row * seq + real_last) * hidden;
                let mut vec: Vec<f32> = data[base..base + hidden].to_vec();
                let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt() + 1e-12;
                for x in vec.iter_mut() {
                    *x /= norm;
                }
                out.push(vec);
            }
        }

        Ok(out)
    }
}
