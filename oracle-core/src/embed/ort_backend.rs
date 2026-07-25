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
        let (inner, _load_ms) = OnnxEmbedder::load(model_dir, default_ep()).with_context(|| {
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

/// The execution provider to request for the current platform: the GPU EP where
/// one is wired (macOS → CoreML, Windows → DirectML), else CPU. `ort` soft-falls
/// back to CPU when the GPU EP cannot register (no driver / device unavailable),
/// so requesting the GPU EP unconditionally is safe and yields "GPU when possible,
/// else CPU" — mirroring the old Python `choose_device` behavior. `ORACLE_RS_EP`
/// ("cpu" | "coreml" | "directml") forces a specific EP, like Python's
/// `ORACLE_EMBED_DEVICE` override (a wrong-platform value fails loudly in `load`).
fn default_ep() -> EpArg {
    if let Ok(forced) = std::env::var("ORACLE_RS_EP") {
        match forced.trim().to_ascii_lowercase().as_str() {
            "cpu" => return EpArg::Cpu,
            "coreml" => return EpArg::Coreml,
            "directml" | "dml" => return EpArg::Directml,
            _ => {}
        }
    }
    #[cfg(target_os = "macos")]
    {
        // CoreML CANNOT run this Qwen3 ONNX export: its MIL compiler rejects the
        // model's unbounded/dynamic dimensions ("has unbounded dimension which is
        // not supported") and embedding hard-fails at session build — confirmed
        // live, and already flagged in the spike. (Python's Mac "GPU" was PyTorch
        // MPS, a different runtime that handles dynamic shapes — NOT CoreML via
        // ONNX, which has no working path for this model.) Default to CPU; a Mac
        // GPU path would need the candle-Metal backend, not ort. `ORACLE_RS_EP=coreml`
        // can still force it for experiments.
        EpArg::Cpu
    }
    #[cfg(target_os = "windows")]
    {
        // DirectML supports dynamic dimensions (unlike CoreML), so it should run
        // this export — but it is UNTESTED here (no Windows machine). If it hits
        // the same graph-compile wall, set ORACLE_RS_EP=cpu.
        EpArg::Directml
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        EpArg::Cpu
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
        // Attention-budget enforcement (post-tokenization split) lives inside
        // OnnxEmbedder::embed_batched — the authoritative gate after true seq_len
        // is known. This loop only bounds outer call size + cancel checks.
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(batch_size.max(1)) {
            if cancel.is_cancelled() {
                anyhow::bail!("embedding cancelled after {} texts", vectors.len());
            }
            let mut v = self
                .inner
                .embed_batched(chunk, chunk.len(), cancel)
                .context("ort embed failed")?;
            vectors.append(&mut v);
        }
        Ok(vectors)
    }
}
