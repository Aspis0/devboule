//! Ingestion pipeline: file collection, chunking, and embedding-text assembly.
//!
//! Ports of `oracle/ingestion/*.py`, phase P2 of PLAN.md.

pub mod ast_chunker;
pub mod chunking;
pub mod collect;
pub mod indexer;
pub mod retrieval_text;
