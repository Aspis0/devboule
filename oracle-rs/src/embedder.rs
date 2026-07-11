#[cfg(all(feature = "metal", not(target_os = "macos")))]
compile_error!("the `metal` feature is macOS-only; build without it on this platform");

use anyhow::{Context, Result};
use candle_core::{DType, Device};
use clap::ValueEnum;
use fastembed::Qwen3TextEmbedding;
use serde::Serialize;

pub const MODEL_ID: &str = "Qwen/Qwen3-Embedding-0.6B";
pub const MAX_LENGTH: usize = 8192;

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

/// A loaded model plus how long the load took.
pub struct Loaded {
    pub model: Qwen3TextEmbedding,
    pub load_ms: u128,
}

/// Load the Qwen3 embedding model from the local HF cache.
pub fn load_model(device: &Device, dtype: DType) -> Result<Loaded> {
    let start = std::time::Instant::now();
    let model = Qwen3TextEmbedding::from_hf(MODEL_ID, device, dtype, MAX_LENGTH)
        .with_context(|| format!("failed to load embedding model {MODEL_ID} from HF cache"))?;
    Ok(Loaded {
        model,
        load_ms: start.elapsed().as_millis(),
    })
}

/// Result of an embed call plus the elapsed wall time.
pub struct EmbedResult {
    pub vectors: Vec<Vec<f32>>,
    pub embed_ms: u128,
}

/// Embed `texts` (L2-normalized) and time the call.
///
/// Texts are processed in chunks of `batch_size` to bound per-call memory and
/// avoid the quadratic blowup of padding the whole input to the longest item.
pub fn embed_texts(
    model: &Qwen3TextEmbedding,
    texts: &[String],
    batch_size: usize,
) -> Result<EmbedResult> {
    let start = std::time::Instant::now();
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(batch_size.max(1)) {
        let mut v = model
            .embed(chunk)
            .with_context(|| "embedding texts failed")?;
        vectors.append(&mut v);
    }
    Ok(EmbedResult {
        vectors,
        embed_ms: start.elapsed().as_millis(),
    })
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
    device_arg: DeviceArg,
    dtype_arg: DtypeArg,
    batch_size: usize,
) -> Result<()> {
    let device = resolve_device(device_arg)?;
    let dtype = dtype_arg.to_dtype();

    let raw = std::fs::read_to_string(&texts_file)
        .with_context(|| format!("reading texts file {}", texts_file.display()))?;
    let texts: Vec<String> = serde_json::from_str(&raw)
        .with_context(|| format!("parsing texts JSON from {}", texts_file.display()))?;
    if texts.is_empty() {
        anyhow::bail!("texts file is empty");
    }

    let loaded = load_model(&device, dtype)?;
    eprintln!("model load: {} ms", loaded.load_ms);

    let res = embed_texts(&loaded.model, &texts, batch_size)?;
    let n = texts.len();
    let tps = if res.embed_ms > 0 {
        n as f64 / (res.embed_ms as f64 / 1000.0)
    } else {
        0.0
    };
    eprintln!(
        "embed: {} ms ({} texts, {:.1} texts/sec)",
        res.embed_ms, n, tps
    );

    let dims = res.vectors.first().map(|v| v.len()).unwrap_or(0);
    let out_obj = EmbedOut {
        model: MODEL_ID.to_string(),
        dims,
        vectors: res.vectors,
    };
    let json = serde_json::to_string_pretty(&out_obj)?;
    std::fs::write(&out, json).with_context(|| format!("writing output {}", out.display()))?;
    Ok(())
}

/// `bench` subcommand: load once, embed the file N times, report throughput.
pub async fn cmd_bench(
    texts_file: std::path::PathBuf,
    iters: usize,
    device_arg: DeviceArg,
    dtype_arg: DtypeArg,
    batch_size: usize,
) -> Result<()> {
    let device = resolve_device(device_arg)?;
    let dtype = dtype_arg.to_dtype();

    let raw = std::fs::read_to_string(&texts_file)
        .with_context(|| format!("reading texts file {}", texts_file.display()))?;
    let texts: Vec<String> = serde_json::from_str(&raw)
        .with_context(|| format!("parsing texts JSON from {}", texts_file.display()))?;
    if texts.is_empty() {
        anyhow::bail!("texts file is empty");
    }

    let loaded = load_model(&device, dtype)?;

    let n = texts.len();
    let total_words: usize = texts.iter().map(|t| t.split_whitespace().count()).sum();

    let mut per_iter_ms: Vec<u128> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let res = embed_texts(&loaded.model, &texts, batch_size)?;
        per_iter_ms.push(res.embed_ms);
    }

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

    let summary = BenchSummary {
        model: MODEL_ID.to_string(),
        device: format!("{:?}", device_arg),
        dtype: format!("{:?}", dtype_arg),
        texts: n,
        iters,
        per_iter_ms,
        avg_ms,
        texts_per_sec: tps,
        words: total_words,
        words_per_sec: wps,
    };
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}
