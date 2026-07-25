#[cfg(all(feature = "metal", not(target_os = "macos")))]
compile_error!("the `metal` feature is macOS-only; build without it on this platform");

use anyhow::{Context, Result};
use candle_core::{DType, Device};
use clap::ValueEnum;
use fastembed::Qwen3TextEmbedding;
use serde::Serialize;

use crate::embed::{CancelFlag, CandleEmbedder, Embedder};
use crate::onnx_embedder::{EpArg, OnnxEmbedder, ONNX_MODEL_ID};
use crate::BackendArg;
use std::time::Instant;

pub const MODEL_ID: &str = "Qwen/Qwen3-Embedding-0.6B";

/// Default max tokens per forward-pass sequence (one embed window).
///
/// Prefer [`crate::embed::EMBED_MAX_SEQ_TOKENS`] / `resolve_embed_max_seq_tokens()`.
/// Kept as an alias so older call sites compile. Long texts are windowed +
/// mean-pooled rather than truncated — see `crate::embed::window_text`.
pub const MAX_LENGTH: usize = crate::embed::EMBED_MAX_SEQ_TOKENS;

/// CLI-facing device selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DeviceArg {
    Cpu,
    Metal,
}

/// CLI-facing weight dtype selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DtypeArg {
    F32,
    F16,
}

impl DtypeArg {
    pub fn to_dtype(self) -> DType {
        match self {
            DtypeArg::F32 => DType::F32,
            DtypeArg::F16 => DType::F16,
        }
    }
}

/// Resolve the candle [`Device`] from the CLI arg.
///
/// `--device metal` is only functional on macOS builds compiled with the
/// `metal` feature. Everywhere else it returns a clear error so Windows / non-metal
/// builds can never accidentally name a metal symbol.
pub fn resolve_device(arg: DeviceArg) -> Result<Device> {
    match arg {
        DeviceArg::Cpu => Ok(Device::Cpu),
        DeviceArg::Metal => metal_device(),
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
fn metal_device() -> Result<Device> {
    Device::new_metal(0).with_context(|| "failed to create metal device")
}

#[cfg(not(all(target_os = "macos", feature = "metal")))]
fn metal_device() -> Result<Device> {
    anyhow::bail!("metal not compiled in (build with --features metal on macOS)")
}

/// A loaded raw model plus how long the load took.
///
/// # Caller contract
///
/// The `model` field is a bare [`Qwen3TextEmbedding`] with **no** windowing and
/// **no** attention-budget enforcement. Prefer [`CandleEmbedder`] /
/// [`Embedder`] for any untrusted-length input; do not call `model.embed`
/// directly on arbitrary texts (attention memory scales as `batch × seq_len²`).
pub struct Loaded {
    pub model: Qwen3TextEmbedding,
    pub load_ms: u128,
}

/// Load the Qwen3 embedding model from the local HF cache.
///
/// Max sequence length is [`crate::embed::resolve_embed_max_seq_tokens`]
/// (one window).
///
/// # Caller contract
///
/// Returns a **raw** model. Callers that embed untrusted-length text must use
/// [`CandleEmbedder`] / the [`Embedder`] trait (windowing + mean-pooling +
/// `pack_windows_for_attention`). Direct `model.embed` skips the runtime
/// attention budget and has frozen hosts on long inputs.
pub fn load_model(device: &Device, dtype: DType) -> Result<Loaded> {
    let start = std::time::Instant::now();
    let max_len = crate::embed::resolve_embed_max_seq_tokens();
    let model = Qwen3TextEmbedding::from_hf(MODEL_ID, device, dtype, max_len)
        .with_context(|| format!("failed to load embedding model {MODEL_ID} from HF cache"))?;
    Ok(Loaded {
        model,
        load_ms: start.elapsed().as_millis(),
    })
}

/// Map CLI device/dtype flags to [`CandleEmbedder::load`] args.
fn candle_load_flags(device_arg: DeviceArg, dtype_arg: DtypeArg) -> (bool, bool) {
    let metal = matches!(device_arg, DeviceArg::Metal);
    let f16 = matches!(dtype_arg, DtypeArg::F16);
    (metal, f16)
}

/// Output shape for the `embed` subcommand.
#[derive(Debug, Serialize)]
pub struct EmbedOut {
    pub model: String,
    pub dims: usize,
    pub vectors: Vec<Vec<f32>>,
}

/// Output shape for the `bench` subcommand.
#[derive(Debug, Serialize)]
pub struct BenchSummary {
    pub model: String,
    pub device: String,
    pub dtype: String,
    pub texts: usize,
    pub iters: usize,
    pub per_iter_ms: Vec<u128>,
    pub avg_ms: f64,
    pub texts_per_sec: f64,
    pub words: usize,
    pub words_per_sec: f64,
}

/// `embed` subcommand: JSON array of strings -> JSON vectors file.
pub async fn cmd_embed(
    texts_file: std::path::PathBuf,
    out: std::path::PathBuf,
    backend: BackendArg,
    device_arg: DeviceArg,
    dtype_arg: DtypeArg,
    model_dir: std::path::PathBuf,
    ep: EpArg,
    batch_size: usize,
) -> Result<()> {
    let raw = std::fs::read_to_string(&texts_file)
        .with_context(|| format!("reading texts file {}", texts_file.display()))?;
    let texts: Vec<String> = serde_json::from_str(&raw)
        .with_context(|| format!("parsing texts JSON from {}", texts_file.display()))?;
    if texts.is_empty() {
        anyhow::bail!("texts file is empty");
    }

    if matches!(backend, BackendArg::Onnx) {
        let (mut embedder, load_ms) = OnnxEmbedder::load(model_dir.as_path(), ep)?;
        eprintln!("model load: {} ms", load_ms);
        let start = Instant::now();
        let vectors =
            embedder.embed_batched(&texts, batch_size, &crate::embed::CancelFlag::new())?;
        let embed_ms = start.elapsed().as_millis();
        let n = texts.len();
        let tps = if embed_ms > 0 {
            n as f64 / (embed_ms as f64 / 1000.0)
        } else {
            0.0
        };
        eprintln!("embed: {} ms ({} texts, {:.1} texts/sec)", embed_ms, n, tps);

        let dims = vectors.first().map(|v| v.len()).unwrap_or(0);
        let out_obj = EmbedOut {
            model: ONNX_MODEL_ID.to_string(),
            dims,
            vectors,
        };
        let json = serde_json::to_string_pretty(&out_obj)?;
        std::fs::write(&out, json).with_context(|| format!("writing output {}", out.display()))?;
        return Ok(());
    }

    let (metal, f16) = candle_load_flags(device_arg, dtype_arg);
    let load_start = Instant::now();
    let mut embedder = CandleEmbedder::load(metal, f16).context("loading candle embedder")?;
    let load_ms = load_start.elapsed().as_millis();
    eprintln!("model load: {} ms", load_ms);

    let start = Instant::now();
    let vectors = embedder.embed(&texts, batch_size, &CancelFlag::new())?;
    let embed_ms = start.elapsed().as_millis();
    let n = texts.len();
    let tps = if embed_ms > 0 {
        n as f64 / (embed_ms as f64 / 1000.0)
    } else {
        0.0
    };
    eprintln!("embed: {} ms ({} texts, {:.1} texts/sec)", embed_ms, n, tps);

    let dims = vectors.first().map(|v| v.len()).unwrap_or(0);
    let out_obj = EmbedOut {
        model: MODEL_ID.to_string(),
        dims,
        vectors,
    };
    let json = serde_json::to_string_pretty(&out_obj)?;
    std::fs::write(&out, json).with_context(|| format!("writing output {}", out.display()))?;
    Ok(())
}

/// Build the `bench` JSON summary from collected per-iteration timings.
fn bench_summary(
    model: String,
    device: String,
    dtype: String,
    n: usize,
    iters: usize,
    per_iter_ms: Vec<u128>,
    total_words: usize,
) -> BenchSummary {
    let safe_iters = iters.max(1) as f64;
    let avg_ms = per_iter_ms.iter().sum::<u128>() as f64 / safe_iters;
    let tps = if avg_ms > 0.0 {
        n as f64 / (avg_ms / 1000.0)
    } else {
        0.0
    };
    let wps = if avg_ms > 0.0 {
        total_words as f64 / (avg_ms / 1000.0)
    } else {
        0.0
    };
    BenchSummary {
        model,
        device,
        dtype,
        texts: n,
        iters,
        per_iter_ms,
        avg_ms,
        texts_per_sec: tps,
        words: total_words,
        words_per_sec: wps,
    }
}

/// `bench` subcommand: load once, embed the file N times, report throughput.
pub async fn cmd_bench(
    texts_file: std::path::PathBuf,
    iters: usize,
    backend: BackendArg,
    device_arg: DeviceArg,
    dtype_arg: DtypeArg,
    model_dir: std::path::PathBuf,
    ep: EpArg,
    batch_size: usize,
) -> Result<()> {
    let raw = std::fs::read_to_string(&texts_file)
        .with_context(|| format!("reading texts file {}", texts_file.display()))?;
    let texts: Vec<String> = serde_json::from_str(&raw)
        .with_context(|| format!("parsing texts JSON from {}", texts_file.display()))?;
    if texts.is_empty() {
        anyhow::bail!("texts file is empty");
    }

    let n = texts.len();
    let total_words: usize = texts.iter().map(|t| t.split_whitespace().count()).sum();
    let device_label = format!("{:?}", device_arg);
    let dtype_label = format!("{:?}", dtype_arg);

    let summary = if matches!(backend, BackendArg::Onnx) {
        let (mut embedder, load_ms) = OnnxEmbedder::load(model_dir.as_path(), ep)?;
        eprintln!("model load: {} ms", load_ms);
        let mut per_iter_ms: Vec<u128> = Vec::with_capacity(iters);
        for _ in 0..iters {
            let start = Instant::now();
            let _ = embedder.embed_batched(&texts, batch_size, &crate::embed::CancelFlag::new())?;
            per_iter_ms.push(start.elapsed().as_millis());
        }
        bench_summary(
            ONNX_MODEL_ID.to_string(),
            device_label,
            dtype_label,
            n,
            iters,
            per_iter_ms,
            total_words,
        )
    } else {
        let (metal, f16) = candle_load_flags(device_arg, dtype_arg);
        let mut embedder = CandleEmbedder::load(metal, f16).context("loading candle embedder")?;

        let mut per_iter_ms: Vec<u128> = Vec::with_capacity(iters);
        for _ in 0..iters {
            let start = Instant::now();
            let _ = embedder.embed(&texts, batch_size, &CancelFlag::new())?;
            per_iter_ms.push(start.elapsed().as_millis());
        }
        bench_summary(
            MODEL_ID.to_string(),
            device_label,
            dtype_label,
            n,
            iters,
            per_iter_ms,
            total_words,
        )
    };

    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::embed::{
        expand_texts_to_windows, pool_window_vectors, resolve_embed_window_bytes,
        resolve_embed_window_overlap_bytes,
    };

    /// Pure stand-in for the CLI candle path (`CandleEmbedder::embed`):
    /// window → (fake per-window vectors) → mean-pool. No model load.
    fn protected_cli_path_vectors(texts: &[String]) -> Vec<Vec<f32>> {
        let window_bytes = resolve_embed_window_bytes();
        let overlap = resolve_embed_window_overlap_bytes();
        let (windows, counts) = expand_texts_to_windows(texts, window_bytes, overlap);
        let fake: Vec<Vec<f32>> = windows.iter().map(|_| vec![1.0f32, 0.0]).collect();
        pool_window_vectors(&fake, &counts)
    }

    #[test]
    fn cli_protected_path_is_one_vector_per_text() {
        let texts = vec![
            "short".to_string(),
            "x".repeat(20_000),
            String::new(),
            "mid".to_string(),
            "y".repeat(5_000),
        ];
        let vectors = protected_cli_path_vectors(&texts);
        assert_eq!(
            vectors.len(),
            texts.len(),
            "CLI protected path must yield exactly one vector per input text"
        );
        for (i, v) in vectors.iter().enumerate() {
            assert_eq!(v.len(), 2, "vector {i} must keep fake dim");
        }
    }
}
