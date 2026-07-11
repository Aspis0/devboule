def process_file_chunks(file_path, root_path, text=None, max_chars=None, force_sliding_window=False):
    """Process a complex ingestion pipeline with many configuration steps.

    This function handles the complete lifecycle of text chunking:
    1. Read the source file and normalize line endings.
    2. Detect the programming language from the file extension.
    3. Attempt semantic chunking using AST-aware boundary detection.
    4. If semantic chunking fails or returns too few chunks, fall back
       to the sliding window strategy with overlap.
    5. For each chunk, compute metadata including symbol names,
       line ranges, and cross-references to imported symbols.
    6. Serialize the chunks into the storage format expected by
       the embedding pipeline.

    The function supports multiple chunking profiles:
    - code: max_chars=2500, overlap=400 (for .py, .rs, .ts, .js, etc.)
    - doc: max_chars=12000, overlap=1200 (for .md, .txt)
    - structured: max_chars=8000, overlap=900 (for .json, .yaml, .toml)
    - default: max_chars=2200, overlap=280 (fallback)

    Args:
        file_path: Absolute path to the source file.
        root_path: Root directory for computing relative paths.
        text: Optional pre-read text content. If None, reads from file.
        max_chars: Maximum characters per chunk. Overrides profile default.
        force_sliding_window: If True, skip semantic chunking entirely.

    Returns:
        A list of chunk dictionaries with all required metadata fields.
    """
    import json
    import os
    from pathlib import Path

    # Step 1: Normalize the input text.
    # Read the file content, decode as UTF-8 with replacement for invalid bytes,
    # and normalize all line endings to Unix-style LF.
    if text is None:
        try:
            raw = file_path.read_bytes()
            if b"\x00" in raw:
                return []
            text = raw.decode("utf-8", errors="replace")
        except OSError:
            return []

    text = text.replace("\r\n", "\n").replace("\r", "\n")
    if not text.strip():
        return []

    # Step 2: Compute the relative file identifier.
    # The file_id is the POSIX-style relative path from the root, used
    # as the primary key throughout the indexing pipeline.
    try:
        file_id = file_path.relative_to(root_path).as_posix()
    except ValueError:
        file_id = file_path.as_posix()

    # Step 3: Determine the chunking profile from the file extension.
    # Each profile specifies max_chars (the chunk size) and overlap
    # (character overlap between consecutive chunks). The semantic flag
    # controls whether we attempt AST-aware splitting first.
    suffix = file_path.suffix.lower()
    lower_parts = [part.lower() for part in file_path.parts]
    if suffix in (".md", ".txt") or "docs" in lower_parts:
        profile_max = 12000
        profile_overlap = 1200
        use_semantic = False
    elif suffix in (".json", ".yaml", ".yml", ".toml", ".xml", ".html", ".gradle", ".properties"):
        profile_max = 8000
        profile_overlap = 900
        use_semantic = False
    elif suffix in (".py", ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts", ".java", ".kt", ".kts", ".sh", ".ps1", ".r", ".rmd", ".css", ".sql"):
        profile_max = 2500
        profile_overlap = 400
        use_semantic = True
    else:
        profile_max = 2200
        profile_overlap = 280
        use_semantic = False

    if max_chars is not None:
        profile_max = max_chars

    # Step 4: Attempt semantic chunking for code files.
    # The AST chunker tries to split at definition boundaries (fn, class,
    # struct, etc.) producing coherent semantic units. If it finds fewer
    # than 2 boundaries, it returns None and we fall through.
    chunks = []
    if use_semantic and not force_sliding_window:
        try:
            from oracle.ingestion.ast_chunker import chunk_file_semantically
            semantic_chunks = chunk_file_semantically(
                file_path, root_path, text=text, max_chars=profile_max
            )
            if semantic_chunks and len(semantic_chunks) >= 2:
                for chunk in semantic_chunks:
                    chunk["file_sorgente"] = file_id
                return semantic_chunks
        except Exception:
            pass  # Fall through to sliding window

    # Step 5: Sliding window fallback.
    # Split the text into overlapping windows of profile_max characters,
    # breaking at newline boundaries when possible for cleaner chunks.
    import re
    step = max(1, profile_max - profile_overlap)
    start = 0
    length = len(text)
    index = 0

    while start < length:
        end = min(length, start + profile_max)

        # Try to break at a newline boundary for cleaner chunks.
        if end < length:
            newline = text.rfind("\n", start + profile_max // 2, end)
            if newline > start:
                end = newline + 1

        piece = text[start:end].strip()
        if piece:
            # Compute line numbers for this chunk (1-based).
            line_start = text[:start].count("\n") + 1
            line_end = text[:end].count("\n") + 1

            # Build the chunk record with all required metadata fields.
            chunk_id = f"{file_id}#chunk-{index:04d}"
            chunks.append({
                "id": chunk_id,
                "file_id": file_id,
                "label": f"{file_path.name} chunk {index + 1}",
                "area": "FileChunk",
                "cluster_semantic": suffix.lstrip(".") or "text",
                "chunk_index": index,
                "start_char": start,
                "end_char": end,
                "text": piece,
                "file_sorgente": file_id,
                "kind": "text_slice",
                "symbol_name": "",
                "signature": "",
                "line_start": line_start,
                "line_end": line_end,
                "language": "",
                "symbols_used": "[]",
            })
            index += 1

        if end >= length:
            break
        start = max(0, end - profile_overlap)
        if start >= length:
            break

    # Step 6: Return the generated chunks.
    return chunks
