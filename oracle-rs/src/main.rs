mod embedder;
mod lance;

use clap::{Parser, Subcommand};
use embedder::{DeviceArg, DtypeArg};

#[derive(Parser)]
#[command(name = "oracle-rs", about = "Qwen3 embeddings + LanceDB query spike")]
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
        #[arg(long, value_enum, default_value = "cpu")]
        device: DeviceArg,
        #[arg(long, value_enum, default_value = "f32")]
        dtype: DtypeArg,
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
        #[arg(long, value_enum, default_value = "cpu")]
        device: DeviceArg,
        #[arg(long, value_enum, default_value = "f32")]
        dtype: DtypeArg,
        #[arg(long, default_value_t = 8)]
        batch_size: usize,
    },
    /// Benchmark embedding throughput over a texts file.
    Bench {
        #[arg(long)]
        texts_file: std::path::PathBuf,
        #[arg(long, default_value_t = 3)]
        iters: usize,
        #[arg(long, value_enum, default_value = "cpu")]
        device: DeviceArg,
        #[arg(long, value_enum, default_value = "f32")]
        dtype: DtypeArg,
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
            device,
            dtype,
            batch_size,
        } => embedder::cmd_embed(texts_file, out, device, dtype, batch_size).await,
        Command::Query {
            db,
            query,
            limit,
            device,
            dtype,
            batch_size,
        } => lance::cmd_query(db, query, limit, device, dtype, batch_size).await,
        Command::Bench {
            texts_file,
            iters,
            device,
            dtype,
            batch_size,
        } => embedder::cmd_bench(texts_file, iters, device, dtype, batch_size).await,
    }
}
