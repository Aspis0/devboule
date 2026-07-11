use clap::{Parser, Subcommand};
use oracle_core::embedder::{self, DeviceArg, DtypeArg};
use oracle_core::{lance, onnx_embedder, BackendArg};

#[derive(Parser)]
#[command(
    name = "oracle-cli",
    about = "oracle-core dev CLI: embed / query / bench"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Embed a JSON array of strings to a JSON vectors file.
    Embed {
        #[arg(long)]
        texts_file: std::path::PathBuf,
        #[arg(long)]
        out: std::path::PathBuf,
        #[arg(long, value_enum, default_value = "candle")]
        backend: BackendArg,
        #[arg(long, value_enum, default_value = "cpu")]
        device: DeviceArg,
        #[arg(long, value_enum, default_value = "f32")]
        dtype: DtypeArg,
        #[arg(long, default_value = "models/qwen3-onnx")]
        model_dir: std::path::PathBuf,
        #[arg(long, value_enum, default_value = "cpu")]
        ep: onnx_embedder::EpArg,
        #[arg(long, default_value_t = 8)]
        batch_size: usize,
    },
    /// Embed a query and run a LanceDB nearest-neighbour search.
    Query {
        #[arg(long)]
        db: std::path::PathBuf,
        #[arg(long)]
        query: String,
        #[arg(long, default_value_t = 5)]
        limit: usize,
        #[arg(long, value_enum, default_value = "candle")]
        backend: BackendArg,
        #[arg(long, value_enum, default_value = "cpu")]
        device: DeviceArg,
        #[arg(long, value_enum, default_value = "f32")]
        dtype: DtypeArg,
        #[arg(long, default_value = "models/qwen3-onnx")]
        model_dir: std::path::PathBuf,
        #[arg(long, value_enum, default_value = "cpu")]
        ep: onnx_embedder::EpArg,
        #[arg(long, default_value_t = 8)]
        batch_size: usize,
    },
    /// Benchmark embedding throughput over a texts file.
    Bench {
        #[arg(long)]
        texts_file: std::path::PathBuf,
        #[arg(long, default_value_t = 3)]
        iters: usize,
        #[arg(long, value_enum, default_value = "candle")]
        backend: BackendArg,
        #[arg(long, value_enum, default_value = "cpu")]
        device: DeviceArg,
        #[arg(long, value_enum, default_value = "f32")]
        dtype: DtypeArg,
        #[arg(long, default_value = "models/qwen3-onnx")]
        model_dir: std::path::PathBuf,
        #[arg(long, value_enum, default_value = "cpu")]
        ep: onnx_embedder::EpArg,
        #[arg(long, default_value_t = 8)]
        batch_size: usize,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Embed {
            texts_file,
            out,
            backend,
            device,
            dtype,
            model_dir,
            ep,
            batch_size,
        } => {
            embedder::cmd_embed(
                texts_file, out, backend, device, dtype, model_dir, ep, batch_size,
            )
            .await
        }
        Command::Query {
            db,
            query,
            limit,
            backend,
            device,
            dtype,
            model_dir,
            ep,
            batch_size,
        } => {
            lance::cmd_query(
                db, query, limit, backend, device, dtype, model_dir, ep, batch_size,
            )
            .await
        }
        Command::Bench {
            texts_file,
            iters,
            backend,
            device,
            dtype,
            model_dir,
            ep,
            batch_size,
        } => {
            embedder::cmd_bench(
                texts_file, iters, backend, device, dtype, model_dir, ep, batch_size,
            )
            .await
        }
    }
}
