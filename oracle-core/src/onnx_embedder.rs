use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::ValueEnum;
use ort::session::{builder::GraphOptimizationLevel, Session};
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
    Directml,
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
        let model_file = if variant == "fp32" {
            "model.onnx".to_string()
        } else {
            format!("model_{variant}.onnx")
        };
        let model_path = model_dir.join("onnx").join(model_file);
        let tokenizer_path = model_dir.join("tokenizer.json");

        let session_builder =
            Session::builder().context("failed to create ONNX session builder")?;
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
                    .map_err(|e| {
                        anyhow::anyhow!("failed to register CoreML execution provider: {e}")
                    })?;
            }
        }
        #[cfg(not(target_os = "macos"))]
        if matches!(ep, EpArg::Coreml) {
            anyhow::bail!("--ep coreml is only supported on macOS builds");
        }

        // Windows GPU via DirectML (any DX12 GPU). `with_execution_providers` uses
        // ort's default soft-fallback (error_on_failure = false): if the GPU EP
        // cannot register (no driver, device busy/absent), ort logs a warning and
        // silently proceeds on CPU — so this call effectively never errors for an
        // unavailable GPU, giving us "GPU when possible, else CPU" for free.
        #[cfg(target_os = "windows")]
        {
            use ort::ep;
            if matches!(ep, EpArg::Directml) {
                builder = builder
                    .with_execution_providers([ep::DirectML::default().build()])
                    .map_err(|e| {
                        anyhow::anyhow!("failed to register DirectML execution provider: {e}")
                    })?;
            }
        }
        #[cfg(not(target_os = "windows"))]
        if matches!(ep, EpArg::Directml) {
            anyhow::bail!("--ep directml is only supported on Windows builds");
        }

        let session = builder.commit_from_file(&model_path).with_context(|| {
            format!("failed to build ONNX session from {}", model_path.display())
        })?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            anyhow::anyhow!(
                "failed to load tokenizer from {}: {e}",
                tokenizer_path.display()
            )
        })?;

        let embedder = OnnxEmbedder { session, tokenizer };
        Ok((embedder, start.elapsed().as_millis()))
    }

    /// Embed `texts` in chunks of `batch_size`.
    ///
    /// Pooling is last-real-token (right padding), matching the candle path,
    /// and each vector is L2-normalized.
    ///
    /// Long texts are split into overlapping byte windows (see
    /// [`crate::embed::window_text`]), each embedded, then mean-pooled so the
    /// public API stays 1:1 with no text dropped. Tokenizer truncation is kept
    /// as a safety net at [`crate::embed::resolve_embed_max_seq_tokens`] but
    /// must never fire for well-formed windows (`n_tokens ≤ n_bytes` + specials).
    /// Forward passes are split so `sub_batch.len() × seq_len²` never exceeds
    /// the attention budget (token estimate for packing = window byte length).
    ///
    /// `cancel` is checked between groups and sub-batches (same granularity as
    /// the candle path).
    pub fn embed_batched(
        &mut self,
        texts: &[String],
        batch_size: usize,
        cancel: &crate::embed::CancelFlag,
    ) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let batch_size = batch_size.max(1);
        let max_seq = crate::embed::resolve_embed_max_seq_tokens();
        let budget = crate::embed::resolve_attention_budget();
        let window_bytes = crate::embed::resolve_embed_window_bytes();
        let overlap = crate::embed::resolve_embed_window_overlap_bytes();

        let (windows, counts) =
            crate::embed::expand_texts_to_windows(texts, window_bytes, overlap);
        let window_lens: Vec<usize> = windows.iter().map(|w| w.len()).collect();
        let groups = crate::embed::pack_windows_for_attention(&window_lens, budget);

        // Right-pad to the longest sequence in each batch; hard-cap sequence length
        // as a safety net (windows should already be ≤ max_seq tokens).
        self.tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            direction: PaddingDirection::Right,
            ..Default::default()
        }));
        self.tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: max_seq,
                strategy: TruncationStrategy::LongestFirst,
                stride: 0,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("failed to configure tokenizer truncation: {e}"))?;

        let mut window_out: Vec<Vec<f32>> = Vec::with_capacity(windows.len());
        for group in groups {
            if cancel.is_cancelled() {
                anyhow::bail!(
                    "embedding cancelled after {} windows",
                    window_out.len()
                );
            }
            let group_windows = &windows[group];
            for chunk in group_windows.chunks(batch_size) {
                if cancel.is_cancelled() {
                    anyhow::bail!(
                        "embedding cancelled after {} windows",
                        window_out.len()
                    );
                }
                self.embed_tokenized_attention_safe(chunk, budget, cancel, &mut window_out)?;
            }
        }

        Ok(crate::embed::pool_window_vectors(&window_out, &counts))
    }

    /// Tokenize `texts`, then forward in sub-batches that satisfy the attention budget.
    ///
    /// This is the authoritative gate: caller's char-based estimate can be wrong;
    /// after tokenization the true padded `seq_len` is known and must not be
    /// violated by a forward pass.
    fn embed_tokenized_attention_safe(
        &mut self,
        texts: &[String],
        budget: usize,
        cancel: &crate::embed::CancelFlag,
        out: &mut Vec<Vec<f32>>,
    ) -> Result<()> {
        if texts.is_empty() {
            return Ok(());
        }
        if cancel.is_cancelled() {
            anyhow::bail!("embedding cancelled after {} windows", out.len());
        }

        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let seq_len = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(0)
            .max(1);
        let max_n = crate::embed::max_batch_for_attention(seq_len, budget);

        if texts.len() <= max_n {
            self.forward_encodings(&encodings, seq_len, budget, out)?;
            return Ok(());
        }

        // Re-tokenize smaller groups: sub-batches may pad to a shorter seq_len.
        for sub in texts.chunks(max_n) {
            if cancel.is_cancelled() {
                anyhow::bail!("embedding cancelled after {} windows", out.len());
            }
            self.embed_tokenized_attention_safe(sub, budget, cancel, out)?;
        }
        Ok(())
    }

    fn forward_encodings(
        &mut self,
        encodings: &[tokenizers::Encoding],
        seq_len: usize,
        budget: usize,
        out: &mut Vec<Vec<f32>>,
    ) -> Result<()> {
        let batch = encodings.len();
        // Release-safe attention gate at the forward pass: subdivide rather
        // than assert. `batch == 1` is irreducible (still attempted; bounded
        // by the window / max_seq cap).
        if batch > 1 && crate::embed::attention_cost(batch, seq_len) > budget {
            let sizes = crate::embed::attention_sub_batch_sizes(batch, seq_len, budget);
            let mut off = 0usize;
            for size in sizes {
                let sub = &encodings[off..off + size];
                let sub_seq = sub
                    .iter()
                    .map(|e| e.get_ids().len())
                    .max()
                    .unwrap_or(0)
                    .max(1);
                self.forward_encodings(sub, sub_seq, budget, out)?;
                off += size;
            }
            return Ok(());
        }

        let mut ids_vec: Vec<i64> = Vec::with_capacity(batch * seq_len);
        let mut mask_vec: Vec<i64> = Vec::with_capacity(batch * seq_len);
        let mut pos_vec: Vec<i64> = Vec::with_capacity(batch * seq_len);

        for enc in encodings {
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
        Ok(())
    }
}
