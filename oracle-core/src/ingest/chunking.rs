//! Chunking helpers — build_chunks_for_file, split_text, chunk_limits_for_file.
//!
//! Port of the relevant functions from `oracle/ingestion/chunk_index.py`.

use std::path::Path;

use super::ast_chunker;

// ── Configuration constants (defaults, mirroring oracle/config.py) ───────────

pub const CHUNK_MAX_CHARS: usize = 2200;
pub const CHUNK_OVERLAP_CHARS: usize = 280;
pub const CHUNK_DOC_MAX_CHARS: usize = 12000;
pub const CHUNK_DOC_OVERLAP_CHARS: usize = 1200;
pub const CHUNK_STRUCTURED_MAX_CHARS: usize = 8000;
pub const CHUNK_STRUCTURED_OVERLAP_CHARS: usize = 900;
pub const CHUNK_CODE_MAX_CHARS: usize = 2500;
pub const CHUNK_CODE_OVERLAP_CHARS: usize = 400;
pub const CHUNK_MAX_FILE_BYTES: u64 = 1_200_000;
pub const EMBED_DIMS: usize = 1024;

// ── Extension sets ───────────────────────────────────────────────────────────

pub fn is_text_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".css"
            | ".gradle"
            | ".html"
            | ".java"
            | ".js"
            | ".jsx"
            | ".json"
            | ".jsonc"
            | ".kt"
            | ".kts"
            | ".md"
            | ".mjs"
            | ".cjs"
            | ".mts"
            | ".cts"
            | ".properties"
            | ".ps1"
            | ".py"
            | ".r"
            | ".rmd"
            | ".rs"
            | ".sh"
            | ".sql"
            | ".toml"
            | ".ts"
            | ".tsx"
            | ".xml"
            | ".txt"
            | ".yaml"
            | ".yml"
    )
}

fn is_doc_extension(ext: &str) -> bool {
    matches!(ext, ".md" | ".txt")
}

fn is_structured_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".gradle"
            | ".html"
            | ".json"
            | ".jsonc"
            | ".properties"
            | ".toml"
            | ".xml"
            | ".yaml"
            | ".yml"
    )
}

fn is_code_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".css"
            | ".java"
            | ".js"
            | ".jsx"
            | ".kt"
            | ".kts"
            | ".mjs"
            | ".cjs"
            | ".mts"
            | ".cts"
            | ".ps1"
            | ".py"
            | ".r"
            | ".rmd"
            | ".rs"
            | ".sh"
            | ".sql"
            | ".ts"
            | ".tsx"
    )
}

// ── Chunk limits ─────────────────────────────────────────────────────────────

pub fn chunk_limits_for_file(path: &Path) -> (usize, usize) {
    let suffix = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();

    let lower_parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();

    if is_doc_extension(&suffix) || lower_parts.iter().any(|p| p == "docs") {
        return (CHUNK_DOC_MAX_CHARS, CHUNK_DOC_OVERLAP_CHARS);
    }
    if is_structured_extension(&suffix) {
        return (CHUNK_STRUCTURED_MAX_CHARS, CHUNK_STRUCTURED_OVERLAP_CHARS);
    }
    if is_code_extension(&suffix) {
        return (CHUNK_CODE_MAX_CHARS, CHUNK_CODE_OVERLAP_CHARS);
    }
    (CHUNK_MAX_CHARS, CHUNK_OVERLAP_CHARS)
}

// ── Split text (sliding window) ──────────────────────────────────────────────

pub fn split_text(text: &str, max_chars: usize, overlap: usize) -> Vec<(usize, usize, String)> {
    let clean = text.replace("\r\n", "\n");
    if clean.trim().is_empty() {
        return vec![];
    }

    // Work with chars for correct Unicode handling
    let chars: Vec<char> = clean.chars().collect();
    let length = chars.len();
    let mut chunks = Vec::new();
    let mut start: usize = 0;
    let step = max_chars.saturating_sub(overlap).max(1);

    while start < length {
        let mut end = (start + max_chars).min(length);

        // Newline snap in the back half
        if end < length {
            let search_start = (start + max_chars / 2).min(end);
            // Find the last newline in [search_start, end)
            let mut newline_pos = None;
            for i in (search_start..end).rev() {
                if chars[i] == '\n' {
                    newline_pos = Some(i);
                    break;
                }
            }
            if let Some(nl) = newline_pos {
                if nl > start {
                    end = nl + 1;
                }
            }
        }

        let piece: String = chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
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
        if start < end.saturating_sub(step) {
            start = end.saturating_sub(overlap);
        }
    }

    chunks
}

// ── Read text file ───────────────────────────────────────────────────────────

pub fn read_text_file(path: &Path) -> Option<String> {
    // Refuse non-regular files (devices, dirs) before following content.
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() || meta.file_type().is_symlink() => {}
        _ => return None,
    }
    let raw = std::fs::read(path).ok()?;
    if raw.contains(&0u8) {
        return None;
    }
    Some(String::from_utf8_lossy(&raw).to_string())
}

/// True when `path` resolves to a regular file under `root` (symlink-safe).
fn path_resolves_under_root(path: &Path, root: &Path) -> bool {
    let Ok(canon_root) = std::fs::canonicalize(root) else {
        return false;
    };
    let Ok(canon) = std::fs::canonicalize(path) else {
        return false;
    };
    match std::fs::metadata(&canon) {
        Ok(m) if m.is_file() => canon.starts_with(&canon_root),
        _ => false,
    }
}

// ── Build chunks for file (the main entry point) ────────────────────────────

pub fn build_chunks_for_file(path: &Path, root: &Path) -> Vec<serde_json::Value> {
    // Fail-closed: never index content whose resolved target escapes the workspace.
    if !path_resolves_under_root(path, root) {
        return vec![];
    }
    let text = match read_text_file(path) {
        Some(t) => t,
        None => return vec![],
    };

    let file_id = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");

    let (max_chars, _overlap) = chunk_limits_for_file(path);

    // Try semantic chunking first
    if let Some(semantic_chunks) =
        ast_chunker::chunk_file_semantically(path, root, Some(&text), max_chars)
    {
        return semantic_chunks;
    }

    // Sliding-window fallback
    let suffix = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "text".to_string());

    let cluster_suffix = suffix.trim_start_matches('.').to_string();
    let cluster_semantic = if cluster_suffix.is_empty() {
        "text".to_string()
    } else {
        cluster_suffix
    };

    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let overlap = (max_chars / 8).max(200);
    let pieces = split_text(&text, max_chars, overlap);

    let mut chunks = Vec::new();
    for (index, (start, end, piece)) in pieces.into_iter().enumerate() {
        chunks.push(serde_json::json!({
            "id": format!("{}#chunk-{:04}", file_id, index),
            "file_id": file_id,
            "label": format!("{} chunk {}", file_name, index + 1),
            "area": "FileChunk",
            "cluster_semantic": cluster_semantic,
            "chunk_index": index,
            "start_char": start,
            "end_char": end,
            "text": piece,
            "file_sorgente": file_id,
            "kind": "text_slice",
            "symbol_name": "",
            "signature": "",
            "line_start": 0,
            "line_end": 0,
            "language": "",
            "symbols_used": "[]",
        }));
    }

    chunks
}
