//! ONNX Runtime backend for the [`Embedder`](super::Embedder) trait.
//!
//! Wraps the proven spike code in `crate::onnx_embedder` (manual last-token
//! pooling + empty-KV feeding). fp32 is index-parity-proven (0.9998); int8 is
//! ~2× faster on CPU but parity-INCOMPATIBLE (0.70-0.91) — only for corpora
//! embedded entirely with int8.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::{CancelFlag, Embedder};
use crate::onnx_embedder::{EpArg, OnnxEmbedder};

pub struct OrtEmbedder {
    inner: OnnxEmbedder,
    model_id: String,
}

impl OrtEmbedder {
    /// Load `model_dir/onnx/model.onnx` (fp32) or `model_int8.onnx`.
    pub fn load(model_dir: &Path, int8: bool) -> Result<Self> {
        // The spike selects the graph via ORACLE_RS_ONNX_VARIANT; drive it
        // explicitly here so callers don't depend on ambient env state.
        let variant = if int8 { "int8" } else { "fp32" };
        std::env::set_var("ORACLE_RS_ONNX_VARIANT", variant);
        let (inner, _load_ms) = OnnxEmbedder::load(model_dir, EpArg::Cpu).with_context(|| {
            format!(
                "loading ONNX embedder ({variant}) from {}",
                model_dir.display()
            )
        })?;
        Ok(OrtEmbedder {
            inner,
            model_id: format!("Qwen3-Embedding-0.6B-ONNX-{variant}"),
        })
    }

    /// Default on-disk location for the ONNX model bundle.
    pub fn default_model_dir(oracle_data_root: &Path) -> PathBuf {
        oracle_data_root.join("models").join("qwen3-onnx")
    }
}

impl Embedder for OrtEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed(
        &mut self,
        texts: &[String],
        batch_size: usize,
        cancel: &CancelFlag,
    ) -> Result<Vec<Vec<f32>>> {
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(batch_size.max(1)) {
            if cancel.is_cancelled() {
                anyhow::bail!("embedding cancelled after {} texts", vectors.len());
            }
            let mut v = self
                .inner
                .embed_batched(chunk, chunk.len())
                .context("ort embed failed")?;
            vectors.append(&mut v);
        }
        Ok(vectors)
    }
}
