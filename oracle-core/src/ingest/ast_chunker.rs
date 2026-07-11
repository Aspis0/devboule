//! Semantic-aware chunker — splits source files at definition boundaries.
//!
//! Port of `oracle/ingestion/ast_chunker.py` (bug-for-bug, no improvements).

use regex::Regex;
use std::path::Path;

// ── Language detection ───────────────────────────────────────────────────────

fn lang_by_ext(suffix: &str) -> &'static str {
    match suffix {
        ".rs" => "rust",
        ".py" => "python",
        ".ts" | ".tsx" => "typescript",
        ".js" | ".jsx" | ".mjs" | ".cjs" => "javascript",
        ".mts" | ".cts" => "typescript",
        ".java" => "java",
        ".kt" | ".kts" => "kotlin",
        ".sh" | ".ps1" => "bash",
        ".r" | ".rmd" => "r",
        ".sql" => "sql",
        ".css" => "css",
        ".html" => "html",
        ".json" | ".jsonc" => "json",
        ".yaml" | ".yml" => "yaml",
        ".toml" => "toml",
        ".xml" => "xml",
        ".md" | ".txt" => "markdown",
        ".gradle" => "gradle",
        ".properties" => "text",
        _ => "text",
    }
}

pub fn detect_language(file_path: &Path) -> &'static str {
    match file_path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let lower = ext.to_lowercase();
            lang_by_ext(&format!(".{}", lower))
        }
        None => "text",
    }
}

// ── Definition boundary detection ────────────────────────────────────────────

struct DefPattern {
    re: Regex,
    kind: &'static str,
}

fn definition_patterns_for(language: &str) -> Vec<DefPattern> {
    match language {
        "rust" => vec![
            DefPattern { re: Regex::new(r"^\s*(pub(?:\s*\(\s*crate\s*\))?\s+)?fn\s+([A-Za-z_]\w*)").unwrap(), kind: "function" },
            DefPattern { re: Regex::new(r"^\s*(pub\s+)?(unsafe\s+)?(async\s+)?fn\s+([A-Za-z_]\w*)").unwrap(), kind: "function" },
            DefPattern { re: Regex::new(r"^\s*(pub\s+)?struct\s+([A-Za-z_]\w*)").unwrap(), kind: "struct" },
            DefPattern { re: Regex::new(r"^\s*(pub\s+)?enum\s+([A-Za-z_]\w*)").unwrap(), kind: "enum" },
            DefPattern { re: Regex::new(r"^\s*(pub\s+)?trait\s+([A-Za-z_]\w*)").unwrap(), kind: "trait" },
            DefPattern { re: Regex::new(r"^\s*(pub\s+)?(unsafe\s+)?impl\b").unwrap(), kind: "impl" },
            DefPattern { re: Regex::new(r"^\s*(pub\s+)?mod\s+([A-Za-z_]\w*)").unwrap(), kind: "module" },
            DefPattern { re: Regex::new(r"^\s*(pub\s+)?type\s+([A-Za-z_]\w*)").unwrap(), kind: "type" },
            DefPattern { re: Regex::new(r"^\s*macro_rules!\s+([A-Za-z_]\w*)").unwrap(), kind: "macro" },
        ],
        "python" => vec![
            DefPattern { re: Regex::new(r"^\s*def\s+([A-Za-z_]\w*)\s*\(").unwrap(), kind: "function" },
            DefPattern { re: Regex::new(r"^\s*async\s+def\s+([A-Za-z_]\w*)\s*\(").unwrap(), kind: "function" },
            DefPattern { re: Regex::new(r"^\s*class\s+([A-Za-z_]\w*)\s*[:(]").unwrap(), kind: "class" },
        ],
        "typescript" => vec![
            DefPattern { re: Regex::new(r"^\s*(export\s+)?(async\s+)?function\s+([A-Za-z_$][\w$]*)").unwrap(), kind: "function" },
            DefPattern { re: Regex::new(r"^\s*(export\s+)?(const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?\(").unwrap(), kind: "function" },
            DefPattern { re: Regex::new(r"^\s*(export\s+)?class\s+([A-Za-z_$][\w$]*)").unwrap(), kind: "class" },
            DefPattern { re: Regex::new(r"^\s*(export\s+)?(interface|type)\s+([A-Za-z_$][\w$]*)").unwrap(), kind: "type" },
            DefPattern { re: Regex::new(r"^\s*(export\s+)?(enum|namespace)\s+([A-Za-z_$][\w$]*)").unwrap(), kind: "type" },
        ],
        "javascript" => vec![
            DefPattern { re: Regex::new(r"^\s*(export\s+)?(async\s+)?function\s+([A-Za-z_$][\w$]*)").unwrap(), kind: "function" },
            DefPattern { re: Regex::new(r"^\s*(export\s+)?(const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?\(").unwrap(), kind: "function" },
            DefPattern { re: Regex::new(r"^\s*(export\s+)?class\s+([A-Za-z_$][\w$]*)").unwrap(), kind: "class" },
        ],
        "java" => vec![
            DefPattern { re: Regex::new(r"^\s*(public|private|protected)?\s*(static\s+)?(class|interface|enum)\s+([A-Za-z_]\w*)").unwrap(), kind: "class" },
            DefPattern { re: Regex::new(r"^\s*(public|private|protected)?\s*(static\s+)?[\w<>\[\],\s]+\s+([A-Za-z_]\w*)\s*\(").unwrap(), kind: "function" },
        ],
        "kotlin" => vec![
            DefPattern { re: Regex::new(r"^\s*fun\s+([A-Za-z_]\w*)").unwrap(), kind: "function" },
            DefPattern { re: Regex::new(r"^\s*class\s+([A-Za-z_]\w*)").unwrap(), kind: "class" },
            DefPattern { re: Regex::new(r"^\s*interface\s+([A-Za-z_]\w*)").unwrap(), kind: "interface" },
            DefPattern { re: Regex::new(r"^\s*object\s+([A-Za-z_]\w*)").unwrap(), kind: "object" },
            DefPattern { re: Regex::new(r"^\s*(data\s+)?class\s+([A-Za-z_]\w*)").unwrap(), kind: "class" },
            DefPattern { re: Regex::new(r"^\s*(sealed\s+)?(class|interface)\s+([A-Za-z_]\w*)").unwrap(), kind: "class" },
            DefPattern { re: Regex::new(r"^\s*enum\s+class\s+([A-Za-z_]\w*)").unwrap(), kind: "enum" },
        ],
        _ => vec![],
    }
}

// ── Import/reference patterns for symbols_used ──────────────────────────────

fn import_patterns_for(language: &str) -> Vec<Regex> {
    match language {
        "rust" => vec![
            Regex::new(r"use\s+([\w:]+(?:::\w+)*)").unwrap(),
            Regex::new(r"\b([\w]+)::([\w]+)").unwrap(),
            Regex::new(r"\b([A-Z][\w]*)\b").unwrap(),
        ],
        "python" => vec![
            Regex::new(r"(?:from|import)\s+([\w.]+)").unwrap(),
            Regex::new(r"\b([a-z_][\w_]*)\.([a-zA-Z_]\w*)\s*\(").unwrap(),
        ],
        "typescript" | "javascript" => vec![
            Regex::new(r#"import\s*\{([^}]+)\}\s*from\s*['"]([^'"]+)['"]"#).unwrap(),
            Regex::new(r#"import\s+(\w+)\s+from\s*['"]([^'"]+)['"]"#).unwrap(),
            Regex::new(r#"require\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap(),
        ],
        "java" => vec![Regex::new(r"import\s+([\w.]+)").unwrap()],
        "kotlin" => vec![Regex::new(r"import\s+([\w.]+)").unwrap()],
        _ => vec![],
    }
}

// ── Helper functions ─────────────────────────────────────────────────────────

const KEYWORDS: &[&str] = &[
    "export",
    "default",
    "async",
    "static",
    "public",
    "private",
    "protected",
    "unsafe",
    "const",
    "let",
    "var",
    "function",
];

fn symbol_name_from_match(captures: &regex::Captures, _kind: &str) -> String {
    let mut groups: Vec<&str> = Vec::new();
    for i in 1..captures.len() {
        if let Some(m) = captures.get(i) {
            groups.push(m.as_str());
        }
    }
    if groups.is_empty() {
        return String::new();
    }
    let ident_re = Regex::new(r"^[A-Za-z_$][\w$]*$").unwrap();
    for g in groups.iter().rev() {
        let g = g.trim();
        if !g.is_empty() && !g.starts_with("pub") && !KEYWORDS.contains(&g) && ident_re.is_match(g)
        {
            return g.to_string();
        }
    }
    String::new()
}

fn extract_signature(text: &str, symbol_name: &str) -> String {
    for line in text.lines() {
        let stripped = line.trim();
        if !symbol_name.is_empty() && stripped.contains(symbol_name) {
            return stripped.chars().take(200).collect();
        }
    }
    text.lines()
        .next()
        .map(|l| l.trim().chars().take(200).collect())
        .unwrap_or_default()
}

fn extract_symbols_used(text: &str, language: &str) -> Vec<String> {
    let patterns = import_patterns_for(language);
    let mut symbols = std::collections::HashSet::new();
    for pattern in &patterns {
        for caps in pattern.captures_iter(text) {
            if caps.len() == 1 {
                if let Some(m) = caps.get(1) {
                    let g = m.as_str().trim();
                    if !g.is_empty() && !g.starts_with('.') && g.len() >= 2 {
                        symbols.insert(g.to_string());
                    }
                }
            } else {
                for i in 1..caps.len() {
                    if let Some(m) = caps.get(i) {
                        let g = m.as_str().trim();
                        if !g.is_empty() && !g.starts_with('.') && g.len() >= 2 {
                            symbols.insert(g.to_string());
                        }
                    }
                }
            }
        }
    }
    let mut result: Vec<String> = symbols.into_iter().collect();
    result.sort();
    result.truncate(30);
    result
}

fn serialize_symbols(symbols: &[String]) -> String {
    // Match Python's json.dumps format: ["a", "b"] (space after comma)
    let json = serde_json::to_string(symbols).unwrap_or_else(|_| "[]".to_string());
    json.replace(',', ", ")
}

// ── Char-position helpers ────────────────────────────────────────────────────

/// Compute start character position of each line (character-based, not byte-based).
fn compute_line_char_positions(lines: &[String]) -> Vec<usize> {
    let mut positions = vec![0usize];
    let mut pos = 0usize;
    for line in lines.iter().take(lines.len().saturating_sub(1)) {
        pos += line.chars().count() + 1; // +1 for newline
        positions.push(pos);
    }
    positions
}

// ── Semantic chunking engine ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SemanticChunk {
    pub start_char: usize,
    pub end_char: usize,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub line_start: usize,
    pub line_end: usize,
    pub language: String,
    pub symbols_used: Vec<String>,
    pub text: String,
}

pub fn split_semantic(text: &str, language: &str, max_chars: usize) -> Vec<SemanticChunk> {
    if text.trim().is_empty() {
        return vec![];
    }

    // Normalize CRLF → LF (matching Python: text.replace("\r\n", "\n").replace("\r", "\n"))
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");

    let patterns = definition_patterns_for(language);
    if patterns.is_empty() {
        return fallback_chunks(&normalized, language, max_chars);
    }

    let lines: Vec<String> = normalized.split('\n').map(|s| s.to_string()).collect();

    struct Boundary {
        line_idx: usize,
        kind: String,
        name: String,
        indent: usize,
    }

    // Scan for definition boundaries
    let mut boundaries: Vec<Boundary> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with("//") || stripped.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        for pat in &patterns {
            if let Some(m) = pat.re.captures(line) {
                let name = symbol_name_from_match(&m, pat.kind);
                boundaries.push(Boundary {
                    line_idx: i,
                    kind: pat.kind.to_string(),
                    name,
                    indent,
                });
                break;
            }
        }
    }

    if boundaries.is_empty() {
        return fallback_chunks(&normalized, language, max_chars);
    }

    // Filter to top-level boundaries using indent stack
    struct TopLevel {
        line_idx: usize,
        kind: String,
        name: String,
    }

    let mut top_level: Vec<TopLevel> = Vec::new();
    let mut indent_stack: Vec<usize> = Vec::new();

    for b in &boundaries {
        while let Some(&top) = indent_stack.last() {
            if b.indent <= top {
                indent_stack.pop();
            } else {
                break;
            }
        }
        if indent_stack.is_empty() {
            top_level.push(TopLevel {
                line_idx: b.line_idx,
                kind: b.kind.clone(),
                name: b.name.clone(),
            });
        }
        indent_stack.push(b.indent);
    }

    if top_level.is_empty() {
        return fallback_chunks(&normalized, language, max_chars);
    }

    // Build chunks from top-level boundaries
    let char_positions = compute_line_char_positions(&lines);
    let mut chunks: Vec<SemanticChunk> = Vec::new();

    for (idx, tl) in top_level.iter().enumerate() {
        let end_line = if idx + 1 < top_level.len() {
            top_level[idx + 1].line_idx
        } else {
            lines.len()
        };

        let start_char = char_positions[tl.line_idx];

        // end_char: matches Python's formula exactly
        let end_line_clamped = end_line.min(lines.len() - 1);
        let prev_line_idx = if end_line > 0 {
            (end_line - 1).min(lines.len() - 1)
        } else {
            0
        };
        let end_char = char_positions[end_line_clamped] + lines[prev_line_idx].chars().count();

        let chunk_text: String = lines[tl.line_idx..end_line].join("\n");

        // If too large, sub-split
        if chunk_text.chars().count() > max_chars * 2 {
            let sub_chunks = subsplit_large(
                &chunk_text,
                &lines[tl.line_idx..end_line],
                start_char,
                &tl.kind,
                &tl.name,
                language,
                max_chars,
            );
            chunks.extend(sub_chunks);
            continue;
        }

        chunks.push(SemanticChunk {
            start_char,
            end_char,
            kind: tl.kind.clone(),
            symbol_name: tl.name.clone(),
            signature: extract_signature(&chunk_text, &tl.name),
            line_start: tl.line_idx + 1,
            line_end: end_line,
            language: language.to_string(),
            symbols_used: extract_symbols_used(&chunk_text, language),
            text: chunk_text,
        });
    }

    // Add preamble chunk for text before the first top-level definition
    if let Some(first) = top_level.first() {
        if first.line_idx > 0 {
            let preamble_text: String = lines[..first.line_idx].join("\n");
            let trimmed = preamble_text.trim();
            if !trimmed.is_empty() && trimmed.chars().count() > 40 {
                let preamble_end = char_positions[first.line_idx];
                let pre_symbols = extract_symbols_used(trimmed, language);
                chunks.insert(
                    0,
                    SemanticChunk {
                        start_char: 0,
                        end_char: preamble_end,
                        kind: "module_header".to_string(),
                        symbol_name: String::new(),
                        signature: String::new(),
                        line_start: 1,
                        line_end: boundaries[0].line_idx,
                        language: language.to_string(),
                        symbols_used: pre_symbols,
                        text: trimmed.to_string(),
                    },
                );
            }
        }
    }

    chunks
}

fn subsplit_large(
    chunk_text: &str,
    chunk_lines: &[String],
    base_offset: usize,
    kind: &str,
    name: &str,
    language: &str,
    max_chars: usize,
) -> Vec<SemanticChunk> {
    if chunk_lines.is_empty() {
        return vec![];
    }

    let char_offsets = compute_line_char_positions(chunk_lines);
    let mut sub_chunks: Vec<SemanticChunk> = Vec::new();
    let mut current_start: usize = 0;
    let mut current_group: Vec<String> = Vec::new();
    let mut current_chars: usize = 0;

    for (i, line) in chunk_lines.iter().enumerate() {
        let line_chars = line.chars().count() + 1;

        let should_break = (line.trim().is_empty() && current_chars > max_chars / 2)
            || (current_chars + line_chars > max_chars && !current_group.is_empty());

        if should_break {
            let sub_text = current_group.join("\n");
            let start_char = base_offset + char_offsets[current_start];
            let end_char = base_offset + char_offsets[i];

            let symbol_name = if !name.is_empty() {
                format!("{}#part{}", name, sub_chunks.len() + 1)
            } else {
                String::new()
            };

            sub_chunks.push(SemanticChunk {
                start_char,
                end_char,
                kind: kind.to_string(),
                symbol_name,
                signature: String::new(),
                line_start: current_start + 1,
                line_end: i,
                language: language.to_string(),
                symbols_used: extract_symbols_used(&sub_text, language),
                text: sub_text,
            });
            current_start = i;
            current_group = Vec::new();
            current_chars = 0;
        }

        current_group.push(line.clone());
        current_chars += line_chars;
    }

    // Final group
    if !current_group.is_empty() {
        let sub_text = current_group.join("\n");
        let start_char = base_offset + char_offsets[current_start];
        let end_char = base_offset + chunk_text.chars().count();

        let symbol_name = if !name.is_empty() {
            format!("{}#part{}", name, sub_chunks.len() + 1)
        } else {
            String::new()
        };

        sub_chunks.push(SemanticChunk {
            start_char,
            end_char,
            kind: kind.to_string(),
            symbol_name,
            signature: String::new(),
            line_start: current_start + 1,
            line_end: chunk_lines.len(),
            language: language.to_string(),
            symbols_used: extract_symbols_used(&sub_text, language),
            text: sub_text,
        });
    }

    sub_chunks
}

fn fallback_chunks(text: &str, language: &str, max_chars: usize) -> Vec<SemanticChunk> {
    let lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
    let mut chunks: Vec<SemanticChunk> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_chars: usize = 0;
    let mut char_pos: usize = 0;

    let heading_re = Regex::new(r"^(#{1,6})\s").unwrap();

    for (i, line) in lines.iter().enumerate() {
        let line_chars = line.chars().count() + 1;
        let heading = if let Some(m) = heading_re.captures(line) {
            m.get(1).unwrap().as_str().len()
        } else {
            0
        };

        let should_break = (heading > 0 && current_chars > 100)
            || (line.trim().is_empty() && current_chars > max_chars / 2 && !current.is_empty())
            || (current_chars + line_chars > max_chars && !current.is_empty());

        if should_break {
            let chunk_text = current.join("\n");
            let consumed: usize = current.iter().map(|l| l.chars().count() + 1).sum();
            let start_char = char_pos.saturating_sub(consumed);
            let line_start = i + 1 - current.len();
            let line_end = i;

            chunks.push(SemanticChunk {
                start_char,
                end_char: char_pos,
                kind: "section".to_string(),
                symbol_name: String::new(),
                signature: String::new(),
                line_start,
                line_end,
                language: language.to_string(),
                symbols_used: vec![],
                text: chunk_text,
            });
            current = Vec::new();
            current_chars = 0;
        }

        current.push(line.clone());
        current_chars += line_chars;
        char_pos += line_chars;
    }

    if !current.is_empty() {
        let chunk_text = current.join("\n");
        let consumed: usize = current.iter().map(|l| l.chars().count() + 1).sum();
        let start_char = char_pos.saturating_sub(consumed);
        let line_start = lines.len() + 1 - current.len();
        let line_end = lines.len();

        chunks.push(SemanticChunk {
            start_char,
            end_char: char_pos,
            kind: "section".to_string(),
            symbol_name: String::new(),
            signature: String::new(),
            line_start,
            line_end,
            language: language.to_string(),
            symbols_used: vec![],
            text: chunk_text,
        });
    }

    chunks
}

// ── Public API ───────────────────────────────────────────────────────────────

const SEMANTIC_SKIP_LANGUAGES: &[&str] = &[
    "text", "json", "yaml", "toml", "xml", "html", "css", "markdown", "gradle", "sql", "r",
];

pub fn chunk_file_semantically(
    path: &Path,
    root: &Path,
    text: Option<&str>,
    max_chars: usize,
) -> Option<Vec<serde_json::Value>> {
    let language = detect_language(path);

    if SEMANTIC_SKIP_LANGUAGES.contains(&language) {
        return None;
    }

    let text_owned;
    let text = match text {
        Some(t) => t,
        None => {
            text_owned = match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(_) => return None,
            };
            &text_owned
        }
    };

    let chunks = split_semantic(text, language, max_chars);

    if chunks.is_empty() || chunks.len() < 2 {
        return None;
    }

    let file_id = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");

    let file_name = path.file_name().unwrap().to_string_lossy().to_string();

    let mut result: Vec<serde_json::Value> = Vec::new();
    for (idx, c) in chunks.iter().enumerate() {
        let label = if c.symbol_name.is_empty() {
            format!("{} chunk {}", file_name, idx + 1)
        } else {
            c.symbol_name.clone()
        };

        result.push(serde_json::json!({
            "id": format!("{}#chunk-{:04}", file_id, idx),
            "file_id": file_id,
            "label": label,
            "area": format!("FileChunk:{}", c.kind),
            "cluster_semantic": format!("{}:{}", c.language, c.kind),
            "chunk_index": idx,
            "start_char": c.start_char,
            "end_char": c.end_char,
            "text": c.text,
            "file_sorgente": file_id,
            "kind": c.kind,
            "symbol_name": c.symbol_name,
            "signature": c.signature,
            "line_start": c.line_start,
            "line_end": c.line_end,
            "language": c.language,
            "symbols_used": serialize_symbols(&c.symbols_used),
        }));
    }

    Some(result)
}
