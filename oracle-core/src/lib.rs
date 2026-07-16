//! oracle-core: Rust-native Oracle runtime (indexing, retrieval, answering).
//!
//! Promoted from the `oracle-rs` spike (2026-07-11). Module layout grows per
//! PLAN.md phases: store/, ingest/, query/, answer/, jobs, server, doctor.
//! For now it hosts the proven embedding backends and the LanceDB query path.

pub mod cluster;
pub mod config;
pub mod doctor;
pub mod embed;
pub mod embedder;
pub mod ingest;
pub mod jobs;
pub mod lance;
pub mod model_download;
pub mod onnx_embedder;
pub mod server;
pub mod store;
pub mod watch;

use clap::ValueEnum;

/// Embedding backend selector shared by the CLI and (later) runtime config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendArg {
    Candle,
    Onnx,
}
pub mod answer;
pub mod mcp;
pub mod query;
