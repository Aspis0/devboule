//! candle (fastembed qwen3) backend for the [`Embedder`](super::Embedder) trait.
//!
//! Wraps the proven spike code in `crate::embedder` (Qwen3TextEmbedding via
//! the shared HF cache, last-token pooling + L2 norm — index-parity 0.9998
//! against the Python sentence-transformers stack).

use anyhow::{Context, Result};
use candle_core::DType;

use super::{CancelFlag, Embedder};
use crate::embedder::{load_model, resolve_device, DeviceArg, MAX_LENGTH, MODEL_ID};

pub struct CandleEmbedder {
    model: fastembed::Qwen3TextEmbedding,
}

impl CandleEmbedder {
    /// Load from the HF cache. `metal`/`f16` select the device/dtype pair;
    /// non-metal loads always use F32 (F16 on CPU is slower, not faster).
    pub fn load(metal: bool, f16: bool) -> Result<Self> {
        let device = resolve_device(if metal {
            DeviceArg::Metal
        } else {
            DeviceArg::Cpu
        })?;
        let dtype = if metal && f16 { DType::F16 } else { DType::F32 };
        let loaded = load_model(&device, dtype).context("loading candle Qwen3 model")?;
        Ok(CandleEmbedder {
            model: loaded.model,
        })
    }
}

impl Embedder for CandleEmbedder {
    fn model_id(&self) -> &str {
        MODEL_ID
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
            // Truncation is silent inside the model (MAX_LENGTH tokens); warn
            // on inputs that are certainly over the cap (≈4 chars/token).
            for t in chunk {
                if t.chars().count() > MAX_LENGTH * 8 {
                    eprintln!(
                        "warning: text of {} chars will be truncated to {MAX_LENGTH} tokens",
                        t.chars().count()
                    );
                }
            }
            let mut v = self.model.embed(chunk).context("candle embed failed")?;
            vectors.append(&mut v);
        }
        Ok(vectors)
    }
}
