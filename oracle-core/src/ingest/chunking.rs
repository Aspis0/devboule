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

/// Overlap used when hard-splitting a single oversize line/run, matching the
/// non-semantic path in [`build_chunks_for_file`].
pub fn hard_split_overlap(max_chars: usize) -> usize {
    let max_chars = max_chars.max(1);
    (max_chars / 8).max(200).min(max_chars.saturating_sub(1))
}

/// Hard-split `text` on char boundaries into pieces of at most `max_chars`
/// characters with `overlap` character overlap. Never emits a piece longer
/// than `max_chars`. Returns `(start_char, end_char, piece)` relative to `text`.
///
/// Used when a single line exceeds the file-type limit (line-oriented splitters
/// cannot break it) and as the core of [`split_text`] for runs without newlines.
pub fn hard_split_chars(
    text: &str,
    max_chars: usize,
    overlap: usize,
) -> Vec<(usize, usize, String)> {
    let max_chars = max_chars.max(1);
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![];
    }
    if chars.len() <= max_chars {
        return vec![(0, chars.len(), text.to_string())];
    }

    let mut chunks = Vec::new();
    let mut start: usize = 0;
    while start < chars.len() {
        let end = (start + max_chars).min(chars.len());
        let piece: String = chars[start..end].iter().collect();
        chunks.push((start, end, piece));
        if end >= chars.len() {
            break;
        }
        let mut next = end.saturating_sub(overlap);
        if next <= start {
            // Ensure progress even with extreme overlap.
            next = start + 1;
        }
        start = next;
    }
    chunks
}

pub fn split_text(text: &str, max_chars: usize, overlap: usize) -> Vec<(usize, usize, String)> {
    let clean = text.replace("\r\n", "\n");
    if clean.trim().is_empty() {
        return vec![];
    }

    let max_chars = max_chars.max(1);
    // Work with chars for correct Unicode handling
    let chars: Vec<char> = clean.chars().collect();
    let length = chars.len();
    let mut chunks = Vec::new();
    let mut start: usize = 0;

    while start < length {
        let mut end = (start + max_chars).min(length);

        // Newline snap in the back half (prefer soft boundaries when present).
        if end < length {
            let search_start = (start + max_chars / 2).min(end);
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

        // Guarantee: never emit a piece longer than max_chars (hard-split if
        // the chosen span still exceeds — should not happen after the min above,
        // but keep the invariant explicit for a single long line).
        if end - start > max_chars {
            end = start + max_chars;
        }

        let piece: String = chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        if !piece.is_empty() {
            // piece may be shorter than [start,end) after trim; still record
            // the char span used for overlap advance.
            chunks.push((start, end, piece));
        }

        if end >= length {
            break;
        }
        let mut next = end.saturating_sub(overlap);
        if next <= start {
            next = start + 1;
        }
        start = next;
    }

    chunks
}

#[cfg(test)]
mod split_text_tests {
    use super::*;

    #[test]
    fn hard_split_single_line_40k_respects_max_chars() {
        let max_chars = CHUNK_CODE_MAX_CHARS;
        let overlap = hard_split_overlap(max_chars);
        let original: String = (0..40_000)
            .map(|i| char::from(b'A' + (i % 26) as u8))
            .collect();
        let pieces = hard_split_chars(&original, max_chars, overlap);
        assert!(pieces.len() > 1);
        for (_, _, p) in &pieces {
            assert!(
                p.chars().count() <= max_chars,
                "piece has {} chars > max_chars={max_chars}",
                p.chars().count()
            );
        }
        // Reconstruct with overlap removed via char offsets.
        let mut rebuilt = String::new();
        let mut cursor = 0usize;
        for (s, e, _) in &pieces {
            if *e <= cursor {
                continue;
            }
            let from = cursor.saturating_sub(*s);
            let piece_chars: Vec<char> = original.chars().skip(*s).take(e - s).collect();
            let take: String = piece_chars.into_iter().skip(from).collect();
            rebuilt.push_str(&take);
            cursor = *e;
        }
        assert_eq!(rebuilt, original);
    }

    #[test]
    fn split_text_single_line_40k_respects_max_chars() {
        let max_chars = CHUNK_CODE_MAX_CHARS;
        let overlap = CHUNK_CODE_OVERLAP_CHARS;
        let original: String = (0..40_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let pieces = split_text(&original, max_chars, overlap);
        assert!(pieces.len() > 1);
        for (_, _, p) in &pieces {
            assert!(
                p.chars().count() <= max_chars,
                "split_text piece has {} chars",
                p.chars().count()
            );
        }
        // Offsets must cover the full range with only overlap gaps.
        assert_eq!(pieces[0].0, 0);
        assert_eq!(pieces.last().unwrap().1, original.chars().count());
    }
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
