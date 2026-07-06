"""Semantic-aware chunker — splits source files at definition boundaries
(functions, classes, structs, modules) instead of arbitrary character offsets.

Each chunk is a coherent semantic unit with structured metadata:
  kind, symbol_name, signature, line_range, language, symbols_used.

NO external dependencies — pure regex, works across all supported languages."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any


# ── Language detection ───────────────────────────────────────────────────────

LANG_BY_EXT: dict[str, str] = {
    ".rs": "rust",
    ".py": "python",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".js": "javascript",
    ".jsx": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".mts": "typescript",
    ".cts": "typescript",
    ".java": "java",
    ".kt": "kotlin",
    ".kts": "kotlin",
    ".sh": "bash",
    ".ps1": "bash",
    ".r": "r",
    ".rmd": "r",
    ".sql": "sql",
    ".css": "css",
    ".html": "html",
    ".json": "json",
    ".jsonc": "json",
    ".yaml": "yaml",
    ".yml": "yaml",
    ".toml": "toml",
    ".xml": "xml",
    ".md": "markdown",
    ".txt": "markdown",
    ".gradle": "gradle",
    ".properties": "text",
}


def detect_language(file_path: str | Path) -> str:
    suffix = Path(file_path).suffix.lower()
    return LANG_BY_EXT.get(suffix, "text")


# ── Definition boundary detection ────────────────────────────────────────────

# Each pattern is a (regex, kind) tuple. The regex must match the START of a
# definition line.  Capturing group 1 = symbol name (when present).
DEFINITION_PATTERNS: dict[str, list[tuple[re.Pattern[str], str]]] = {
    "rust": [
        (
            re.compile(r"^\s*(pub(?:\s*\(\s*crate\s*\))?\s+)?fn\s+([A-Za-z_]\w*)"),
            "function",
        ),
        (
            re.compile(r"^\s*(pub\s+)?(unsafe\s+)?(async\s+)?fn\s+([A-Za-z_]\w*)"),
            "function",
        ),
        (re.compile(r"^\s*(pub\s+)?struct\s+([A-Za-z_]\w*)"), "struct"),
        (re.compile(r"^\s*(pub\s+)?enum\s+([A-Za-z_]\w*)"), "enum"),
        (re.compile(r"^\s*(pub\s+)?trait\s+([A-Za-z_]\w*)"), "trait"),
        (re.compile(r"^\s*(pub\s+)?(unsafe\s+)?impl\b"), "impl"),
        (re.compile(r"^\s*(pub\s+)?mod\s+([A-Za-z_]\w*)"), "module"),
        (re.compile(r"^\s*(pub\s+)?type\s+([A-Za-z_]\w*)"), "type"),
        (re.compile(r"^\s*macro_rules!\s*([A-Za-z_]\w*)"), "macro"),
    ],
    "python": [
        (re.compile(r"^\s*def\s+([A-Za-z_]\w*)\s*\("), "function"),
        (re.compile(r"^\s*async\s+def\s+([A-Za-z_]\w*)\s*\("), "function"),
        (re.compile(r"^\s*class\s+([A-Za-z_]\w*)\s*[:(]"), "class"),
    ],
    "typescript": [
        (
            re.compile(r"^\s*(export\s+)?(async\s+)?function\s+([A-Za-z_$][\w$]*)"),
            "function",
        ),
        (
            re.compile(
                r"^\s*(export\s+)?(const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?\("
            ),
            "function",
        ),
        (re.compile(r"^\s*(export\s+)?class\s+([A-Za-z_$][\w$]*)"), "class"),
        (re.compile(r"^\s*(export\s+)?(interface|type)\s+([A-Za-z_$][\w$]*)"), "type"),
        (re.compile(r"^\s*(export\s+)?(enum|namespace)\s+([A-Za-z_$][\w$]*)"), "type"),
    ],
    "javascript": [
        (
            re.compile(r"^\s*(export\s+)?(async\s+)?function\s+([A-Za-z_$][\w$]*)"),
            "function",
        ),
        (
            re.compile(
                r"^\s*(export\s+)?(const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?\("
            ),
            "function",
        ),
        (re.compile(r"^\s*(export\s+)?class\s+([A-Za-z_$][\w$]*)"), "class"),
    ],
    "java": [
        (
            re.compile(
                r"^\s*(public|private|protected)?\s*(static\s+)?(class|interface|enum)\s+([A-Za-z_]\w*)"
            ),
            "class",
        ),
        (
            re.compile(
                r"^\s*(public|private|protected)?\s*(static\s+)?[\w<>[\],\s]+\s+([A-Za-z_]\w*)\s*\("
            ),
            "function",
        ),
    ],
    "kotlin": [
        (re.compile(r"^\s*fun\s+([A-Za-z_]\w*)"), "function"),
        (re.compile(r"^\s*class\s+([A-Za-z_]\w*)"), "class"),
        (re.compile(r"^\s*interface\s+([A-Za-z_]\w*)"), "interface"),
        (re.compile(r"^\s*object\s+([A-Za-z_]\w*)"), "object"),
        (re.compile(r"^\s*(data\s+)?class\s+([A-Za-z_]\w*)"), "class"),
        (re.compile(r"^\s*(sealed\s+)?(class|interface)\s+([A-Za-z_]\w*)"), "class"),
        (re.compile(r"^\s*enum\s+class\s+([A-Za-z_]\w*)"), "enum"),
    ],
}


# Cross-language import/reference patterns for extracting symbols_used
IMPORT_REFERENCE_PATTERNS: dict[str, list[re.Pattern[str]]] = {
    "rust": [
        re.compile(r"use\s+([\w:]+(?:::\w+)*)"),
        re.compile(r"\b([\w]+)::([\w]+)"),
        re.compile(r"\b([A-Z][\w]*)\b"),  # type names
    ],
    "python": [
        re.compile(r"(?:from|import)\s+([\w.]+)"),
        re.compile(r"\b([a-z_][\w_]*)\.([a-zA-Z_]\w*)\s*\("),
    ],
    "typescript": [
        re.compile(r"import\s*\{([^}]+)\}\s*from\s*['\"]([^'\"]+)['\"]"),
        re.compile(r"import\s+(\w+)\s+from\s*['\"]([^'\"]+)['\"]"),
        re.compile(r"require\s*\(\s*['\"]([^'\"]+)['\"]"),
    ],
    "javascript": [
        re.compile(r"import\s*\{([^}]+)\}\s*from\s*['\"]([^'\"]+)['\"]"),
        re.compile(r"import\s+(\w+)\s+from\s*['\"]([^'\"]+)['\"]"),
        re.compile(r"require\s*\(\s*['\"]([^'\"]+)['\"]"),
    ],
    "java": [
        re.compile(r"import\s+([\w.]+)"),
    ],
    "kotlin": [
        re.compile(r"import\s+([\w.]+)"),
    ],
}


def _symbol_name_from_match(match: re.Match[str], kind: str) -> str:
    """Extract the symbol name from a definition pattern match.
    Different languages/patterns capture the name in different groups."""
    groups = [g for g in match.groups() if g is not None]
    if not groups:
        return ""
    # The name is typically the last non-keyword group
    for g in reversed(groups):
        g = str(g).strip()
        if (
            g
            and not g.startswith("pub")
            and g
            not in (
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
            )
        ):
            if re.match(r"^[A-Za-z_$][\w$]*$", g):
                return g
    return ""


def _extract_signature(text: str, symbol_name: str, language: str) -> str:
    """Extract a readable one-line signature for the symbol."""
    lines = text.split("\n")
    for line in lines:
        stripped = line.strip()
        if symbol_name and symbol_name in stripped:
            # Return first line containing the symbol name
            return stripped[:200]
    return lines[0].strip()[:200] if lines else ""


def _count_lines(text: str, start_pos: int) -> int:
    """Count newlines from position 0 to start_pos (1-based line number)."""
    return text[:start_pos].count("\n") + 1 if start_pos > 0 else 1


def _line_range(text: str, start: int, end: int) -> tuple[int, int]:
    return (_count_lines(text, start), _count_lines(text, end))


def _extract_symbols_used(text: str, language: str) -> list[str]:
    """Extract external symbols used in a chunk (imports, type references, calls)."""
    patterns = IMPORT_REFERENCE_PATTERNS.get(language, [])
    symbols: set[str] = set()
    for pattern in patterns:
        for match in pattern.findall(text):
            if isinstance(match, tuple):
                for g in match:
                    g = str(g).strip()
                    if g and not g.startswith(".") and len(g) >= 2:
                        symbols.add(g)
            else:
                g = str(match).strip()
                if g and not g.startswith(".") and len(g) >= 2:
                    symbols.add(g)
    return sorted(symbols)[:30]


# ── Chunking engine ──────────────────────────────────────────────────────────


def split_semantic(
    text: str, language: str, max_chars: int = 2500
) -> list[dict[str, Any]]:
    """Split source text into semantic chunks at definition boundaries.

    Strategy:
      1. Normalize line endings (\r\n → \n)
      2. Scan for definition boundaries (fn, class, struct, etc.)
      3. Filter out nested boundaries (inner defs stay inside their parent)
      4. Group lines between top-level boundaries into chunks
      5. If a single definition exceeds max_chars, sub-split at logical points
      6. For languages without definition patterns, fall back gracefully

    Returns list of dicts with:
      start_char, end_char, kind, symbol_name, signature,
      line_start, line_end, language
    """
    if not text or not text.strip():
        return []

    # Normalize CRLF → LF before any line-based processing
    text = text.replace("\r\n", "\n").replace("\r", "\n")

    patterns = DEFINITION_PATTERNS.get(language, [])
    # Fallback for non-code languages: split at blank-line / heading boundaries
    if not patterns:
        return _fallback_chunks(text, language, max_chars)

    lines = text.split("\n")
    # Find definition boundaries (line indices) with their indentation level
    boundaries: list[
        tuple[int, str, str, int]
    ] = []  # (line_idx, kind, symbol_name, indent)
    for i, line in enumerate(lines):
        stripped = line.strip()
        if not stripped or stripped.startswith("//") or stripped.startswith("#"):
            continue
        indent = len(line) - len(line.lstrip())
        for pattern, kind in patterns:
            m = pattern.match(line)
            if m:
                name = _symbol_name_from_match(m, kind)
                boundaries.append((i, kind, name, indent))
                break

    if not boundaries:
        # No definitions found — use paragraph-based fallback
        return _fallback_chunks(text, language, max_chars)

    # Filter: keep only TOP-LEVEL boundaries (not nested inside another definition).
    # A boundary at line L with indent D is top-level if all previous boundaries
    # with lower line numbers have indent >= D (i.e. it's not indented more than
    # the currently-open definition). We track the current nesting via a stack.
    top_level: list[tuple[int, str, str]] = []
    indent_stack: list[int] = []  # indentation of open definitions
    for line_idx, kind, name, indent in boundaries:
        # Pop definitions that have ended (we're at same or lower indent)
        while indent_stack and indent <= indent_stack[-1]:
            indent_stack.pop()
        # This is top-level if the stack is now empty
        if not indent_stack:
            top_level.append((line_idx, kind, name))
        indent_stack.append(indent)

    if not top_level:
        return _fallback_chunks(text, language, max_chars)

    # Build chunks from top-level boundaries
    chunks: list[dict[str, Any]] = []
    char_positions = _compute_char_positions(text, lines)

    for idx, (start_line, kind, name) in enumerate(top_level):
        end_line = top_level[idx + 1][0] if idx + 1 < len(top_level) else len(lines)
        start_char = char_positions[start_line]
        end_char = char_positions[min(end_line, len(lines) - 1)] + len(
            lines[min(end_line - 1, len(lines) - 1)]
        )

        chunk_text = "\n".join(lines[start_line:end_line])
        # If too large, sub-split at secondary boundaries
        if len(chunk_text) > max_chars * 2:
            sub_chunks = _subsplit_large(
                chunk_text,
                lines[start_line:end_line],
                start_char,
                kind,
                name,
                language,
                max_chars,
            )
            chunks.extend(sub_chunks)
            continue

        chunks.append(
            {
                "start_char": start_char,
                "end_char": end_char,
                "kind": kind,
                "symbol_name": name,
                "signature": _extract_signature(chunk_text, name, language),
                "line_start": start_line + 1,
                "line_end": end_line,
                "language": language,
                "symbols_used": _extract_symbols_used(chunk_text, language),
                "text": chunk_text,
            }
        )

    # Add a preamble chunk for text before the first top-level definition
    if top_level and top_level[0][0] > 0:
        preamble_lines = lines[: top_level[0][0]]
        preamble_text = "\n".join(preamble_lines).strip()
        if preamble_text and len(preamble_text) > 40:
            preamble_end = char_positions[top_level[0][0]]
            pre_symbols = _extract_symbols_used(preamble_text, language)
            chunks.insert(
                0,
                {
                    "start_char": 0,
                    "end_char": preamble_end,
                    "kind": "module_header",
                    "symbol_name": "",
                    "signature": "",
                    "line_start": 1,
                    "line_end": boundaries[0][0],
                    "language": language,
                    "symbols_used": pre_symbols,
                    "text": preamble_text,
                },
            )

    return chunks


def _compute_char_positions(text: str, lines: list[str]) -> list[int]:
    """Compute start character position of each line."""
    positions = [0]
    pos = 0
    for line in lines[:-1]:
        pos += len(line) + 1  # +1 for newline
        positions.append(pos)
    return positions


def _subsplit_large(
    chunk_text: str,
    chunk_lines: list[str],
    base_offset: int,
    kind: str,
    name: str,
    language: str,
    max_chars: int,
) -> list[dict[str, Any]]:
    """Split an oversized definition chunk at logical sub-boundaries
    (blank lines, comment blocks, or statement groups)."""
    if not chunk_lines:
        return []

    sub_chunks: list[dict[str, Any]] = []
    current_start = 0
    current_group: list[str] = []
    current_chars = 0
    # Build char positions relative to base_offset
    char_offsets = _compute_char_positions(chunk_text, chunk_lines)

    for i, line in enumerate(chunk_lines):
        line_chars = len(line) + 1  # +1 for newline
        # Break at blank lines or when group is full
        if (not line.strip() and current_chars > max_chars // 2) or (
            current_chars + line_chars > max_chars and current_group
        ):
            sub_text = "\n".join(current_group)
            start_char = base_offset + char_offsets[current_start]
            end_char = base_offset + char_offsets[i]
            sub_chunks.append(
                {
                    "start_char": start_char,
                    "end_char": end_char,
                    "kind": kind,
                    "symbol_name": f"{name}#part{len(sub_chunks) + 1}" if name else "",
                    "signature": "",
                    "line_start": current_start + 1,
                    "line_end": i,
                    "language": language,
                    "symbols_used": _extract_symbols_used(sub_text, language),
                    "text": sub_text,
                }
            )
            current_start = i
            current_group = []
            current_chars = 0

        current_group.append(line)
        current_chars += line_chars

    # Final group
    if current_group:
        sub_text = "\n".join(current_group)
        start_char = base_offset + char_offsets[current_start]
        end_char = base_offset + len(chunk_text)
        sub_chunks.append(
            {
                "start_char": start_char,
                "end_char": end_char,
                "kind": kind,
                "symbol_name": f"{name}#part{len(sub_chunks) + 1}" if name else "",
                "signature": "",
                "line_start": current_start + 1,
                "line_end": len(chunk_lines),
                "language": language,
                "symbols_used": _extract_symbols_used(sub_text, language),
                "text": sub_text,
            }
        )

    return sub_chunks


def _fallback_chunks(text: str, language: str, max_chars: int) -> list[dict[str, Any]]:
    """Fallback chunking for non-code files: split at blank lines (paragraphs)
    or heading markers (markdown)."""
    lines = text.split("\n")
    chunks: list[dict[str, Any]] = []
    current: list[str] = []
    current_chars = 0
    char_pos = 0

    def _heading_level(line: str) -> int:
        m = re.match(r"^(#{1,6})\s", line)
        return len(m.group(1)) if m else 0

    for i, line in enumerate(lines):
        line_chars = len(line) + 1
        heading = _heading_level(line)
        # Break at headings or blank-line boundaries when group is non-trivial
        if (
            (heading and current_chars > 100)
            or (not line.strip() and current_chars > max_chars // 2 and current)
            or (current_chars + line_chars > max_chars and current)
        ):
            chunk_text = "\n".join(current)
            start_char = char_pos - sum(len(l) + 1 for l in current)
            chunks.append(
                {
                    "start_char": start_char,
                    "end_char": char_pos,
                    "kind": "section",
                    "symbol_name": "",
                    "signature": "",
                    "line_start": i - len(current) + 1,
                    "line_end": i,
                    "language": language,
                    "symbols_used": [],
                    "text": chunk_text,
                }
            )
            current = []
            current_chars = 0

        current.append(line)
        current_chars += line_chars
        char_pos += line_chars

    if current:
        chunk_text = "\n".join(current)
        start_char = char_pos - sum(len(l) + 1 for l in current)
        chunks.append(
            {
                "start_char": start_char,
                "end_char": char_pos,
                "kind": "section",
                "symbol_name": "",
                "signature": "",
                "line_start": len(lines) - len(current) + 1,
                "line_end": len(lines),
                "language": language,
                "symbols_used": [],
                "text": chunk_text,
            }
        )

    return chunks


# ── Public API ───────────────────────────────────────────────────────────────


def _serialize_symbols(symbols: list[str]) -> str:
    import json

    return json.dumps(symbols)


def chunk_file_semantically(
    path: Path,
    root: Path,
    text: str | None = None,
    max_chars: int = 2500,
) -> list[dict[str, Any]] | None:
    """Semantic chunking for a file. Returns None if fallback to sliding window
    is preferred (e.g., for non-code files where semantic chunking adds no value).

    Returns list of chunk dicts with metadata: start_char, end_char, kind,
    symbol_name, signature, line_start, line_end, language, symbols_used, text.
    """
    language = detect_language(path)
    if language in (
        "text",
        "json",
        "yaml",
        "toml",
        "xml",
        "html",
        "css",
        "markdown",
        "gradle",
        "sql",
        "r",
    ):
        return None  # Let sliding window handle these

    if text is None:
        try:
            text = path.read_text(encoding="utf-8")
        except Exception:
            return None

    try:
        chunks = split_semantic(text, language, max_chars)
    except Exception:
        return None

    if not chunks or len(chunks) < 2:
        return None  # Too few chunks → sliding window was fine

    # Compute file_id relative to root
    try:
        file_id = path.relative_to(root).as_posix()
    except ValueError:
        file_id = path.as_posix()

    # Transform into chunk_index-compatible format
    result = []
    for idx, c in enumerate(chunks):
        result.append(
            {
                "id": f"{file_id}#chunk-{idx:04d}",
                "file_id": file_id,
                "label": c.get("symbol_name") or f"{path.name} chunk {idx + 1}",
                "area": f"FileChunk:{c['kind']}",
                "cluster_semantic": f"{c['language']}:{c['kind']}",
                "chunk_index": idx,
                "start_char": c["start_char"],
                "end_char": c["end_char"],
                "text": c["text"],
                "file_sorgente": file_id,
                # ── New structured metadata ──
                "kind": c["kind"],
                "symbol_name": c["symbol_name"],
                "signature": c.get("signature", ""),
                "line_start": c["line_start"],
                "line_end": c["line_end"],
                "language": c["language"],
                "symbols_used": _serialize_symbols(c.get("symbols_used", [])),
            }
        )
    return result
