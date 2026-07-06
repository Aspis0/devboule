"""Phase 1 — Generic structural extractive synthesis for `ask`.

Groups retrieved chunks by file and emits a clean, deterministic answer
using only the structural metadata already present in each chunk
(kind, symbol_name, signature, text). No LLM, no apology text.

Returns the same dict shape as the other answer builders:
    {summary, answer, citations, answer_source, not_found, fallback_reason}
"""

from __future__ import annotations

from oracle.server.answerer import (
    MAX_ANSWER_CHARS,
    not_found_answer,
    truncate_text,
)


def structural_extractive_answer(
    query: str, context: list[dict], reason: str | None = None
) -> dict:
    """Produce a clean extractive answer grouped by file.

    Uses only the chunk metadata (kind, symbol_name, signature, text) to build
    a structured summary. Deterministic — no set-ordering leakage, no LLM call.
    """
    if not context:
        return not_found_answer(query, context, reason=reason)

    # Group chunks by file_source (preserve insertion order — Python 3.7+).
    by_file: dict[str, list[dict]] = {}
    for chunk in context:
        file_source = str(
            chunk.get("file_source") or chunk.get("file_sorgente") or "unknown"
        )
        by_file.setdefault(file_source, []).append(chunk)

    blocks: list[str] = []
    citations = []
    for file_path, chunks in by_file.items():
        lines = [f"📄 `{file_path}`"]
        for chunk in chunks:
            kind = str(chunk.get("kind") or "text_slice")
            symbol = str(chunk.get("symbol_name") or "")
            sig = str(chunk.get("signature") or "")
            text = str(chunk.get("text") or "").strip()

            # Build a compact symbol line.
            if symbol:
                symbol_line = f"  - **{symbol}** ({kind})"
                if sig:
                    # Show first ~120 chars of signature.
                    sig_preview = sig.split("\n")[0].strip()[:120]
                    symbol_line += f": `{sig_preview}`"
            else:
                # Fallback: first non-comment line of text.
                first_line = ""
                for line in text.splitlines()[:3]:
                    stripped = line.strip()
                    if stripped and not stripped.startswith(("//", "#", "*", "/*")):
                        first_line = stripped[:120]
                        break
                symbol_line = f"  - ({kind})"
                if first_line:
                    symbol_line += f": `{first_line}`"

            lines.append(symbol_line)
            citations.append(
                {
                    "ref": chunk.get("ref", ""),
                    "file_source": file_path,
                    "chunk_id": chunk.get("chunk_id", ""),
                    "chunk_index": chunk.get("chunk_index"),
                    "start_char": chunk.get("start_char"),
                    "end_char": chunk.get("end_char"),
                    "retrieval": chunk.get("retrieval", ""),
                    "score": chunk.get("score", 0),
                }
            )

        blocks.append("\n".join(lines))

    body = "\n\n".join(blocks)

    # Build summary from file/symbol overview.
    files = list(by_file.keys())
    symbols = []
    for chunks in by_file.values():
        for c in chunks:
            s = str(c.get("symbol_name") or "")
            if s and s not in symbols:
                symbols.append(s)

    if symbols:
        summary = f"Found relevant code in {len(files)} file(s) covering: {', '.join(symbols[:5])}."
    else:
        summary = f"Found relevant context in {len(files)} file(s)."

    return {
        "summary": summary,
        "answer": truncate_text(body, MAX_ANSWER_CHARS),
        "citations": citations,
        "answer_source": "extractive_synthesis",
        "not_found": False,
        "suggested_path": None,
        "fallback_reason": reason or "structural_extractive",
    }
