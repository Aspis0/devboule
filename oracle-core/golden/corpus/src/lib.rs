//! Oracle core ingestion pipeline for Aspis Management.
//!
//! This module handles text chunking, embedding generation,
//! and semantic indexing of source code files.

use std::collections::HashMap;
use std::path::PathBuf;

/// Configuration for the chunking pipeline.
#[derive(Debug, Clone)]
pub struct ChunkConfig {
    /// Maximum characters per chunk.
    pub max_chars: usize,
    /// Overlap between consecutive chunks.
    pub overlap_chars: usize,
    /// Whether to use semantic (AST-aware) chunking.
    pub use_semantic: bool,
}

impl ChunkConfig {
    /// Create a new configuration with default values.
    pub fn default() -> Self {
        Self {
            max_chars: 2500,
            overlap_chars: 400,
            use_semantic: true,
        }
    }

    /// Override max_chars for code files.
    pub fn code_profile() -> Self {
        Self {
            max_chars: 2500,
            overlap_chars: 400,
            use_semantic: true,
        }
    }

    /// Override max_chars for documentation files.
    pub fn doc_profile() -> Self {
        Self {
            max_chars: 12000,
            overlap_chars: 1200,
            use_semantic: false,
        }
    }
}

/// Supported chunk kinds produced by the semantic chunker.
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Macro,
    Class,
    Type,
    TextSlice,
    ModuleHeader,
}

impl ChunkKind {
    /// Return a human-readable label for the chunk kind.
    pub fn label(&self) -> &str {
        match self {
            ChunkKind::Function => "function",
            ChunkKind::Struct => "struct",
            ChunkKind::Enum => "enum",
            ChunkKind::Trait => "trait",
            ChunkKind::Impl => "impl",
            ChunkKind::Module => "module",
            ChunkKind::Macro => "macro",
            ChunkKind::Class => "class",
            ChunkKind::Type => "type",
            ChunkKind::TextSlice => "text_slice",
            ChunkKind::ModuleHeader => "module_header",
        }
    }
}

/// A single chunk produced by the chunking pipeline.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Unique identifier for this chunk.
    pub id: String,
    /// The source file path relative to the root.
    pub file_id: String,
    /// Index of this chunk within the file.
    pub chunk_index: usize,
    /// Start character offset in the file.
    pub start_char: usize,
    /// End character offset in the file.
    pub end_char: usize,
    /// The text content of the chunk.
    pub text: String,
    /// The kind of semantic unit this chunk represents.
    pub kind: ChunkKind,
    /// Name of the symbol if applicable.
    pub symbol_name: String,
    /// Language of the source file.
    pub language: String,
    /// Starting line number (1-based).
    pub line_start: usize,
    /// Ending line number (1-based).
    pub line_end: usize,
}

/// Build the embedding text for a chunk by prepending metadata headers.
pub fn chunk_embedding_text(chunk: &Chunk) -> String {
    let mut parts = vec![
        "TASK: retrieve Aspis Bio and Aspis Management code/docs chunks.".to_string(),
        format!("SOURCE_PATH: {}", chunk.file_id),
        format!("CHUNK_KIND: {}", chunk.kind.label()),
        format!("SYMBOL_NAME: {}", chunk.symbol_name),
        format!("LANGUAGE: {}", chunk.language),
        format!("LINE_RANGE: L{}-L{}", chunk.line_start, chunk.line_end),
        "RAW_CHUNK:".to_string(),
        chunk.text.clone(),
    ];
    parts.join("\n")
}

/// Split text into overlapping windows of max_chars.
pub fn split_text(text: &str, max_chars: usize, overlap: usize) -> Vec<(usize, usize, String)> {
    let clean = text.replace("\r\n", "\n");
    if clean.trim().is_empty() {
        return vec![];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    let length = clean.len();

    while start < length {
        let mut end = (start + max_chars).min(length);
        if end < length {
            if let Some(newline) = clean[start + max_chars / 2..end].rfind('\n') {
                end = start + max_chars / 2 + newline + 1;
            }
        }
        let piece = clean[start..end].trim().to_string();
        if !piece.is_empty() {
            chunks.push((start, end, piece));
        }
        if end >= length {
            break;
        }
        start = end.saturating_sub(overlap);
        if start >= length {
            break;
        }
    }
    chunks
}

/// Create a macro for generating chunk metadata.
macro_rules! chunk_meta {
    ($file_id:expr, $index:expr) => {{
        format!("{}#chunk-{:04}", $file_id, $index)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_text() {
        let text = "Hello\nWorld\nThis is a test";
        let chunks = split_text(text, 10, 2);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_kind_label() {
        assert_eq!(ChunkKind::Function.label(), "function");
        assert_eq!(ChunkKind::Struct.label(), "struct");
    }

    #[test]
    fn test_default_config() {
        let config = ChunkConfig::default();
        assert_eq!(config.max_chars, 2500);
        assert_eq!(config.overlap_chars, 400);
        assert!(config.use_semantic);
    }
}
