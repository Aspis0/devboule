"""Oracle ingestion pipeline - text chunking and semantic indexing.

This module provides the core text processing functions for splitting
source code into semantically meaningful chunks and generating
embedding-compatible text representations.
"""

from __future__ import annotations

import os
import re
from pathlib import Path
from typing import Optional


class ChunkMetadata:
    """Metadata associated with a single text chunk.

    Attributes:
        file_id: Relative path of the source file.
        chunk_index: Sequential index within the file.
        kind: Semantic kind of the chunk (function, class, etc.).
        symbol_name: Name of the symbol if applicable.
        language: Programming language of the source file.
        line_start: Starting line number (1-based).
        line_end: Ending line number (1-based).
        symbols_used: List of referenced symbols.
    """

    def __init__(
        self,
        file_id: str,
        chunk_index: int,
        kind: str = "text_slice",
        symbol_name: str = "",
        language: str = "",
        line_start: int = 0,
        line_end: int = 0,
        symbols_used: Optional[list[str]] = None,
    ):
        self.file_id = file_id
        self.chunk_index = chunk_index
        self.kind = kind
        self.symbol_name = symbol_name
        self.language = language
        self.line_start = line_start
        self.line_end = line_end
        self.symbols_used = symbols_used or []

    def to_dict(self) -> dict:
        """Serialize metadata to a dictionary."""
        return {
            "file_id": self.file_id,
            "chunk_index": self.chunk_index,
            "kind": self.kind,
            "symbol_name": self.symbol_name,
            "language": self.language,
            "line_start": self.line_start,
            "line_end": self.line_end,
            "symbols_used": self.symbols_used,
        }


def chunk_id(file_id: str, index: int) -> str:
    """Generate a deterministic chunk identifier."""
    return f"{file_id}#chunk-{index:04d}"


def split_text(
    text: str,
    max_chars: int = 2200,
    overlap: int = 280,
) -> list[tuple[int, int, str]]:
    """Split text into overlapping windows with newline-aware boundaries."""
    clean = text.replace("\r\n", "\n")
    if not clean.strip():
        return []
    chunks = []
    start = 0
    length = len(clean)
    step = max(1, max_chars - overlap)
    while start < length:
        end = min(length, start + max_chars)
        if end < length:
            newline = clean.rfind("\n", start + max_chars // 2, end)
            if newline > start:
                end = newline + 1
        piece = clean[start:end].strip()
        if piece:
            chunks.append((start, end, piece))
        if end >= length:
            break
        start = max(0, end - overlap)
        if start >= length:
            break
    return chunks


def classify_domains(source: str, text: str) -> list[str]:
    """Classify the domain tags for a source/text pair."""
    haystack = f"{source}\n{text}".lower()
    domains = []
    if any(n in haystack for n in ["output_renders", "artifact_url"]):
        domains.append("rnaseq_output_release")
    if "scaleway" in haystack:
        domains.append("scaleway_provider_console")
    if "cloudflare" in haystack:
        domains.append("cloudflare_provider_console")
    if any(n in haystack for n in ["zdr", "gdpr"]):
        domains.append("provider_privacy")
    return domains or ["general"]


def extract_symbols(source: str, text: str) -> list[str]:
    """Extract symbol names from source code text."""
    patterns = [
        re.compile(r"\b(?:pub\s+)?fn\s+([A-Za-z_]\w*)"),
        re.compile(r"\b(?:pub\s+)?struct\s+([A-Za-z_]\w*)"),
        re.compile(r"\bdef\s+([A-Za-z_]\w*)\s*\("),
        re.compile(r"\bclass\s+([A-Za-z_]\w*)\s*[:(]"),
    ]
    symbols = []
    seen = set()
    for pattern in patterns:
        for match in pattern.findall(text):
            if match not in seen:
                seen.add(match)
                symbols.append(match)
    return symbols


def format_chunk_for_embedding(
    metadata: ChunkMetadata,
    text: str,
    profile: str = "semantic-prefix-v2",
) -> str:
    """Format a chunk's text with metadata headers for embedding."""
    header_lines = [
        "TASK: retrieve Aspis Bio and Aspis Management code/docs chunks.",
        f"SOURCE_PATH: {metadata.file_id}",
        f"CHUNK_KIND: {metadata.kind}",
        f"SYMBOL_NAME: {metadata.symbol_name}",
        f"LANGUAGE: {metadata.language}",
        f"LINE_RANGE: L{metadata.line_start}-L{metadata.line_end}",
        "RAW_CHUNK:",
    ]
    return "\n".join(header_lines) + "\n" + text


async def compute_embedding_profile(
    file_path: Path,
    profile_name: str = "semantic-prefix-v2",
) -> dict:
    """Compute the embedding profile for a given file asynchronously."""
    suffix = file_path.suffix.lower()
    if suffix in (".md", ".txt"):
        return {"max_chars": 12000, "overlap": 1200, "semantic": False}
    if suffix in (".json", ".yaml", ".yml", ".toml"):
        return {"max_chars": 8000, "overlap": 900, "semantic": False}
    return {"max_chars": 2500, "overlap": 400, "semantic": True}
