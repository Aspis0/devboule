//! candle (fastembed qwen3) backend for the [`Embedder`](super::Embedder) trait.
//!
//! Wraps the proven spike code in `crate::embedder` (Qwen3TextEmbedding via
//! the shared HF cache, last-token pooling + L2 norm — index-parity 0.9998
//! against the Python sentence-transformers stack).

use anyhow::{Context, Result};
use candle_core::DType;

use super::{
    attention_cost, attention_sub_batch_sizes, expand_texts_to_windows, pack_windows_for_attention,
    pool_window_vectors, resolve_attention_budget, resolve_embed_window_bytes,
    resolve_embed_window_overlap_bytes, CancelFlag, Embedder,
};
use crate::embedder::{load_model, resolve_device, DeviceArg, MODEL_ID};

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
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let window_bytes = resolve_embed_window_bytes();
        let overlap = resolve_embed_window_overlap_bytes();
        let budget = resolve_attention_budget();

        // Window every text (no truncation). Public API stays 1:1 via pooling.
        let (windows, counts) = expand_texts_to_windows(texts, window_bytes, overlap);
        let window_lens: Vec<usize> = windows.iter().map(|w| w.len()).collect();
        let groups = pack_windows_for_attention(&window_lens, budget);

        let mut window_vectors: Vec<Vec<f32>> = Vec::with_capacity(windows.len());
        let outer = batch_size.max(1);

        // Walk groups; also respect caller's outer batch_size as a soft cap on
        // how many windows we hand to the model at once (groups already encode
        // the attention budget).
        for group in groups {
            if cancel.is_cancelled() {
                anyhow::bail!(
                    "embedding cancelled after {} windows",
                    window_vectors.len()
                );
            }
            let group_windows = &windows[group.clone()];
            for sub in group_windows.chunks(outer) {
                if cancel.is_cancelled() {
                    anyhow::bail!(
                        "embedding cancelled after {} windows",
                        window_vectors.len()
                    );
                }
                // Release-safe attention gate: subdivide rather than assert.
                // `batch == 1` is irreducible (single sequence still runs;
                // bounded by the window size).
                let est_seq = sub.iter().map(|w| w.len().max(1)).max().unwrap_or(1);
                let sizes = if attention_cost(sub.len(), est_seq) > budget && sub.len() > 1 {
                    attention_sub_batch_sizes(sub.len(), est_seq, budget)
                } else {
                    vec![sub.len()]
                };
                let mut off = 0usize;
                for size in sizes {
                    if cancel.is_cancelled() {
                        anyhow::bail!(
                            "embedding cancelled after {} windows",
                            window_vectors.len()
                        );
                    }
                    let piece = &sub[off..off + size];
                    let mut v = self.model.embed(piece).context("candle embed failed")?;
                    window_vectors.append(&mut v);
                    off += size;
                }
            }
        }

        Ok(pool_window_vectors(&window_vectors, &counts))
    }
}
