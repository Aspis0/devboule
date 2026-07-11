#!/usr/bin/env python3
"""Golden fixture dump harness for the Oracle Rust port.

Calls REAL oracle.* functions (never reimplements) and dumps deterministic
JSON fixtures for byte-parity testing against a future Rust port.

Usage:
    PYTHONPATH=/path/to/Aspis-management \
    python dump_golden.py --corpus corpus/ --queries queries.json --out fixtures/
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Ensure oracle package is importable
# ---------------------------------------------------------------------------
_ASISP = Path(__file__).resolve().parents[2]  # oracle-core/../../ = Aspis-management
if str(_ASISP) not in sys.path:
    sys.path.insert(0, str(_ASISP))

# ---------------------------------------------------------------------------
# Set env vars BEFORE importing oracle modules (module-level constants are
# computed at import time).
# ---------------------------------------------------------------------------
# Disable focused_excerpt truncation so prompts are fully deterministic —
# focused_excerpt uses set iteration order for term positions, which is
# non-deterministic across Python runs.  Setting a huge limit means no
# excerpting occurs; the full chunk text appears in the prompt.
os.environ["ORACLE_ASK_MAX_CHARS_PER_CHUNK"] = "100000"

from oracle.ingestion.chunk_index import (
    build_chunks_for_file,
    collect_text_files,
    priority_rank,
    priority_key,
    read_text_file,
)
from oracle.ingestion.retrieval_text import (
    chunk_embedding_text,
    classify_domains,
    classify_source_kind,
    query_embedding_text,
)
from oracle.server.query_engine import (
    lexical_chunk_context,
    lexical_chunk_score,
    query_terms,
    semantic_expansions,
)
from oracle.server.answerer import (
    build_answer_prompt,
    prepared_context,
    redact_secret_tokens,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_VOLATILE_KEYS = {"ultima_modifica", "embedding_dims"}


def _clean_chunk(chunk: dict) -> dict:
    """Strip volatile fields from a chunk dict for deterministic output."""
    return {k: v for k, v in chunk.items() if k not in _VOLATILE_KEYS}


def _dump(obj, path: Path) -> None:
    """Write JSON with deterministic formatting."""
    with open(path, "w", encoding="utf-8") as f:
        json.dump(obj, f, sort_keys=True, ensure_ascii=False, indent=1)


# ---------------------------------------------------------------------------
# (a) collect.json
# ---------------------------------------------------------------------------

def dump_collect(corpus_root: Path, out_dir: Path) -> list[str]:
    """Dump the ordered list of collected files as relative posix paths.

    Preserves the exact priority order returned by collect_text_files()
    (which sorts by priority_key). Also dumps a parallel priority_rank map.
    """
    files = collect_text_files(corpus_root)
    # Preserve the EXACT order from collect_text_files (already sorted by
    # priority_key). Do NOT re-sort alphabetically — that destroys the
    # priority ordering the Rust port must reproduce.
    rel_paths = [str(p.relative_to(corpus_root).as_posix()) for p in files]
    _dump(rel_paths, out_dir / "collect.json")

    # Parallel map: relpath → priority_rank (integer).
    rank_map = {}
    for rel in rel_paths:
        rank_map[rel] = priority_rank(rel)
    _dump(rank_map, out_dir / "collect_priority.json")

    return rel_paths


# ---------------------------------------------------------------------------
# (b) chunks.json
# ---------------------------------------------------------------------------

def dump_chunks(
    corpus_root: Path, collected: list[str], out_dir: Path
) -> dict[str, list[dict]]:
    """Dump chunks for every collected file, sorted by file_id."""
    all_chunks: dict[str, list[dict]] = {}
    for rel in sorted(collected):
        file_path = corpus_root / rel
        raw_chunks = build_chunks_for_file(file_path, corpus_root)
        clean = [_clean_chunk(c) for c in raw_chunks]
        all_chunks[rel] = clean
    _dump(all_chunks, out_dir / "chunks.json")
    return all_chunks


# ---------------------------------------------------------------------------
# (c) embedding_texts.json
# ---------------------------------------------------------------------------

def dump_embedding_texts(
    all_chunks: dict[str, list[dict]],
    queries: list[str],
    out_dir: Path,
) -> dict:
    """Dump chunk_embedding_text for every chunk and query_embedding_text for every query."""
    chunk_texts: dict[str, str] = {}
    for file_id in sorted(all_chunks):
        for chunk in all_chunks[file_id]:
            cid = chunk["id"]
            chunk_texts[cid] = chunk_embedding_text(chunk)

    query_texts: dict[str, str] = {}
    for q in sorted(queries):
        query_texts[q] = query_embedding_text(q)

    _dump({"chunks": chunk_texts, "queries": query_texts}, out_dir / "embedding_texts.json")
    return {"chunks": chunk_texts, "queries": query_texts}


# ---------------------------------------------------------------------------
# (d) lexical.json
# ---------------------------------------------------------------------------

def dump_lexical(
    all_chunks: dict[str, list[dict]],
    queries: list[str],
    out_dir: Path,
) -> None:
    """Dump lexical_chunk_score for every (query, chunk) pair + semantic expansions."""
    # Flatten all chunks
    flat_chunks = []
    for file_id in sorted(all_chunks):
        flat_chunks.extend(all_chunks[file_id])

    results: dict[str, dict] = {}
    for q in sorted(queries):
        terms = query_terms(q)
        expansions = sorted(semantic_expansions(terms))
        scores: dict[str, float] = {}
        for chunk in flat_chunks:
            score = lexical_chunk_score(q, terms, chunk)
            if score > 0.0:
                scores[chunk["id"]] = round(score, 6)

        # Top-10 lexical ranking
        ranked = sorted(scores.items(), key=lambda x: (-x[1], x[0]))
        top10_ids = [cid for cid, _ in ranked[:10]]

        results[q] = {
            "terms": sorted(terms),
            "semantic_expansions": expansions,
            "chunk_scores": scores,
            "top10_chunk_ids": top10_ids,
        }

    _dump(results, out_dir / "lexical.json")


# ---------------------------------------------------------------------------
# (e) answer_prompt.json
# ---------------------------------------------------------------------------

def dump_answer_prompt(
    all_chunks: dict[str, list[dict]],
    queries: list[str],
    out_dir: Path,
) -> None:
    """Dump the exact prompt string for 3 queries, with fake secrets for redaction testing."""
    # Flatten all chunks
    flat_chunks = []
    for file_id in sorted(all_chunks):
        flat_chunks.extend(all_chunks[file_id])

    # Pick 3 queries that have lexical matches
    selected = []
    for q in queries:
        terms = query_terms(q)
        scored = [(lexical_chunk_score(q, terms, c), c) for c in flat_chunks]
        scored = [(s, c) for s, c in scored if s > 0]
        if scored and len(selected) < 3:
            scored.sort(key=lambda x: (-x[0], x[1]["id"]))
            selected.append((q, [c for _, c in scored[:5]]))

    results = []
    for q, chunks in selected:
        # Build context payload (the shape expected by prepared_context)
        context_for_prepared = []
        for i, chunk in enumerate(chunks):
            # Compute the real lexical_chunk_score for this (query, chunk) pair.
            chunk_lexical_score = lexical_chunk_score(q, query_terms(q), chunk)
            item = {
                "chunk_id": chunk["id"],
                "file_source": chunk.get("file_sorgente", chunk.get("file_id", "")),
                "chunk_index": chunk.get("chunk_index", 0),
                "start_char": chunk.get("start_char", 0),
                "end_char": chunk.get("end_char", 0),
                "text": chunk.get("text", ""),
                "score": round(chunk_lexical_score, 6),
                "retrieval": "lexical",
                "kind": chunk.get("kind", ""),
                "symbol_name": chunk.get("symbol_name", ""),
                "signature": chunk.get("signature", ""),
                "language": chunk.get("language", ""),
                "line_start": chunk.get("line_start", 0),
                "line_end": chunk.get("line_end", 0),
                "symbols_used": chunk.get("symbols_used", "[]"),
            }
            # Inject fake secrets into the first chunk of the first query.
            # Covers ALL SECRET_PATTERNS from oracle/server/answerer.py:
            #   gh*_, github_pat_, SCW*, AKIA*, xox*, Bearer, JWT (eyJ.*),
            #   generic api_key= assignment, plus high-entropy base64 + hex.
            if i == 0 and q == selected[0][0]:
                item["text"] = (
                    item["text"]
                    + "\n# Secrets injection for redaction testing:\n"
                    + "# AWS: AKIAIOSFODNN7EXAMPLE key_id=AKIA1234567890ABCDEF\n"
                    + "# GitHub PAT: github_pat_11ABCDEF0123456789_abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMN\n"
                    + "# GitHub token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef\n"
                    + "# Scaleway: SCWabcdefghijklmnopqrstuvwxyz12\n"
                    + "# Slack: xoxb-123456789012-1234567890123-ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef\n"
                    + "# Bearer: Bearer eyJhbGciOiJIUzI1NiJ9.payload.signature\n"
                    + "# JWT: eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U\n"
                    + "# Generic: api_key=a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6\n"
                    + "# Base64 run: dGhpcyBpcyBhIHZlcnkgbG9uZyBhcml0cmFyeSBhc3NpZ25tZW50IHRoYXQgbmVlZHNSZWRhY3Rpb24K\n"
                    + "# Hex run: 0123456789abcdef0123456789abcdef01234567\n"
                )
            context_for_prepared.append(item)

        prepared = prepared_context(context_for_prepared, q)
        prompt = build_answer_prompt(q, prepared)

        # Also dump the redacted version for reference (only for first query)
        redacted_chunks = None
        if q == selected[0][0]:
            redacted_chunks = []
            for item in prepared:
                redacted_chunks.append({
                    "ref": item["ref"],
                    "chunk_id": item["chunk_id"],
                    "file_source": item["file_source"],
                    "text_redacted": redact_secret_tokens(item["text"]),
                    "text_original": item["text"],
                })

        results.append({
            "query": q,
            "context_chunk_ids": [c["id"] for c in chunks],
            "prompt": prompt,
            "redaction_test": redacted_chunks,
        })

    _dump(results, out_dir / "answer_prompt.json")


# ---------------------------------------------------------------------------
# (f) classify.json
# ---------------------------------------------------------------------------

def dump_classify(
    corpus_root: Path,
    collected: list[str],
    out_dir: Path,
) -> None:
    """Dump classify_domains() and classify_source_kind() for every collected file."""
    results: dict[str, dict] = {}
    for rel in collected:
        file_path = corpus_root / rel
        text = read_text_file(file_path) or ""
        domains = classify_domains(rel, text)
        source_kind = classify_source_kind(rel)
        results[rel] = {
            "domains": sorted(domains),
            "source_kind": source_kind,
        }
    _dump(results, out_dir / "classify.json")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description="Dump golden fixtures from the Oracle Python pipeline")
    parser.add_argument("--corpus", required=True, help="Path to the corpus root directory")
    parser.add_argument("--queries", required=True, help="Path to queries.json")
    parser.add_argument("--out", required=True, help="Output directory for fixtures")
    args = parser.parse_args()

    corpus_root = Path(args.corpus).resolve()
    queries_path = Path(args.queries).resolve()
    out_dir = Path(args.out).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    with open(queries_path, encoding="utf-8") as f:
        queries = json.load(f)

    print(f"Corpus: {corpus_root}")
    print(f"Queries: {len(queries)}")
    print(f"Output: {out_dir}")
    print()

    # (a) collect.json
    print("[a] collect.json ...")
    collected = dump_collect(corpus_root, out_dir)
    print(f"    {len(collected)} files collected")

    # (b) chunks.json
    print("[b] chunks.json ...")
    all_chunks = dump_chunks(corpus_root, collected, out_dir)
    total_chunks = sum(len(cs) for cs in all_chunks.values())
    print(f"    {total_chunks} chunks from {len(all_chunks)} files")

    # (c) embedding_texts.json
    print("[c] embedding_texts.json ...")
    emb = dump_embedding_texts(all_chunks, queries, out_dir)
    print(f"    {len(emb['chunks'])} chunk texts + {len(emb['queries'])} query texts")

    # (d) lexical.json
    print("[d] lexical.json ...")
    dump_lexical(all_chunks, queries, out_dir)
    print("    done")

    # (e) answer_prompt.json
    print("[e] answer_prompt.json ...")
    dump_answer_prompt(all_chunks, queries, out_dir)
    print("    done")

    # (f) classify.json
    print("[f] classify.json ...")
    dump_classify(corpus_root, collected, out_dir)
    print("    done")

    print()
    print(f"Fixtures written to {out_dir}")
    print(f"  collect.json:      {len(collected)} files")
    print(f"  chunks.json:       {total_chunks} chunks")
    print(f"  embedding_texts.json: {len(emb['chunks'])} chunk + {len(emb['queries'])} query texts")
    print(f"  lexical.json:      {len(queries)} queries x {total_chunks} chunks")
    print(f"  answer_prompt.json: 3 prompts with redaction test")
    print(f"  classify.json:     {len(collected)} files classified")


if __name__ == "__main__":
    main()
