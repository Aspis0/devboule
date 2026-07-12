//! Runtime embedding layer: backend trait, per-platform auto-selection, and a
//! resident pool with lazy load / idle unload / cooperative cancellation.
//!
//! This is the piece that replaces the Python resident server's
//! `ingestion/embedder.py` process-level lifecycle: instead of killing a child
//! process to reclaim RAM, the pool drops the model after an idle period and
//! reloads on demand (PLAN.md P3).

mod candle_backend;
pub mod ort_backend;

pub use candle_backend::CandleEmbedder;
pub use ort_backend::OrtEmbedder;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Cooperative cancellation flag, checked between batches.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Which backend the pool should load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendChoice {
    /// candle (fastembed qwen3). `metal` only works on macOS builds with the
    /// `metal` feature; `f16` is only meaningful together with metal.
    Candle { metal: bool, f16: bool },
    /// ONNX Runtime with the platform GPU EP auto-selected (macOS → CoreML,
    /// Windows → DirectML) and automatic CPU fallback. `model_dir` holds
    /// `onnx/model*.onnx` + `tokenizer.json`; `int8` selects the quantized graph
    /// (needs its OWN index — parity-incompatible with f32-embedded corpora).
    Ort { model_dir: PathBuf, int8: bool },
}

/// A loaded embedding backend.
///
/// `embed` must return one L2-normalized vector per input text, in order.
/// Implementations check `cancel` between internal batches and bail with an
/// error containing "cancelled" when it fires.
pub trait Embedder: Send {
    fn model_id(&self) -> &str;
    fn embed(
        &mut self,
        texts: &[String],
        batch_size: usize,
        cancel: &CancelFlag,
    ) -> Result<Vec<Vec<f32>>>;
}

/// Resolve the default backend for this build/platform.
///
/// macOS + `metal` feature → candle Metal F16 (index-parity proven, model in
/// the shared HF cache). Everything else → ONNX int8 with the platform GPU EP
/// auto-selected (CoreML/DirectML) and CPU fallback. `ORACLE_RS_BACKEND=candle|onnx`
/// overrides; `ORACLE_EMBED_DEVICE=cpu` forces CPU on the candle path
/// (mirroring the Python env knob); `ORACLE_RS_EP` forces the ONNX EP.
pub fn default_backend(ort_model_dir: PathBuf) -> BackendChoice {
    let forced = std::env::var("ORACLE_RS_BACKEND").ok();
    let force_cpu = std::env::var("ORACLE_EMBED_DEVICE")
        .map(|v| v.trim().eq_ignore_ascii_case("cpu"))
        .unwrap_or(false);
    let metal_available = cfg!(all(target_os = "macos", feature = "metal")) && !force_cpu;

    match forced.as_deref().map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("onnx") || v.eq_ignore_ascii_case("ort") => {
            BackendChoice::Ort {
                model_dir: ort_model_dir,
                int8: true,
            }
        }
        Some(v) if v.eq_ignore_ascii_case("candle") => BackendChoice::Candle {
            metal: metal_available,
            f16: metal_available,
        },
        _ if metal_available => BackendChoice::Candle {
            metal: true,
            f16: true,
        },
        _ => BackendChoice::Ort {
            model_dir: ort_model_dir,
            int8: true,
        },
    }
}

fn load_backend(choice: &BackendChoice) -> Result<Box<dyn Embedder>> {
    match choice {
        BackendChoice::Candle { metal, f16 } => Ok(Box::new(
            CandleEmbedder::load(*metal, *f16).context("loading candle embedder")?,
        )),
        BackendChoice::Ort { model_dir, int8 } => Ok(Box::new(
            OrtEmbedder::load(model_dir, *int8).context("loading ort embedder")?,
        )),
    }
}

struct PoolState {
    embedder: Option<Box<dyn Embedder>>,
    last_used: Instant,
}

/// Resident embedder pool: lazy load, reuse across calls, idle unload.
pub struct EmbedderPool {
    choice: BackendChoice,
    state: Mutex<PoolState>,
}

impl EmbedderPool {
    pub fn new(choice: BackendChoice) -> Self {
        EmbedderPool {
            choice,
            state: Mutex::new(PoolState {
                embedder: None,
                last_used: Instant::now(),
            }),
        }
    }

    pub fn backend(&self) -> &BackendChoice {
        &self.choice
    }

    /// Whether the model is currently resident in memory.
    pub fn is_loaded(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .embedder
            .is_some()
    }

    /// Embed texts, loading the model on first use.
    ///
    /// The pool lock is held for the whole call: embedding is single-flight by
    /// design (one model instance, GPU/CPU saturating), exactly like the
    /// Python server where one uvicorn worker owned the model.
    pub fn embed(
        &self,
        texts: &[String],
        batch_size: usize,
        cancel: &CancelFlag,
    ) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.embedder.is_none() {
            state.embedder = Some(load_backend(&self.choice)?);
        }
        state.last_used = Instant::now();
        let out = state
            .embedder
            .as_mut()
            .expect("just loaded")
            .embed(texts, batch_size, cancel);
        state.last_used = Instant::now();
        out
    }

    /// Drop the model if it has been idle for at least `max_idle`.
    /// Returns true when an unload happened.
    pub fn unload_if_idle(&self, max_idle: Duration) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.embedder.is_some() && state.last_used.elapsed() >= max_idle {
            state.embedder = None;
            true
        } else {
            false
        }
    }

    /// Drop the model immediately (e.g. on low-memory pressure).
    pub fn unload_now(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.embedder = None;
    }
}
