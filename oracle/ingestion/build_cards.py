"""Phase 2 — Build `node_cards` from `file_chunks` deterministically (no LLM).

Reads chunked file data from SQLite, classifies each file with the existing
`fallback_classification` pipeline, and writes `node_cards` rows + hash vectors
so that `node`/`similar`/`cluster`/`duplicates` graph tools stop being dead.
"""

from __future__ import annotations

from oracle.config import EMBED_DIMS
from oracle.ingestion.classifier import fallback_classification
from oracle.store.lance_store import LanceStore, embed_text
from oracle.store.sqlite_store import SQLiteStore


def build_cards_from_chunks(sqlite_path: str, lance_path: str) -> int:
    """Build one node_cards row per file from existing file_chunks.

    Reads `file_chunks` grouped by `file_id`, reconstructs enough text signal,
    classifies with `fallback_classification` (no LLM), and writes cards to
    SQLite + hash vectors to LanceDB.

    Returns the number of cards written.
    """
    store = SQLiteStore(sqlite_path)
    all_chunks = store.all_chunks()
    if not all_chunks:
        return 0

    # Group chunks by file_id.
    by_file: dict[str, list[dict]] = {}
    for chunk in all_chunks:
        file_id = chunk.get("file_id", "").strip()
        if not file_id:
            continue
        by_file.setdefault(file_id, []).append(chunk)

    cards = []
    for file_id, chunks in by_file.items():
        # Reconstruct file content from chunks (ordered by chunk_index).
        chunks.sort(key=lambda c: c.get("chunk_index", 0))
        content = "\n".join(c.get("text", "") for c in chunks)

        # Gather metadata from chunks for richer classification.
        file_source = chunks[0].get("file_sorgente", file_id)
        ultima_modifica = chunks[0].get("ultima_modifica", "")
        languages = {c.get("language", "") for c in chunks if c.get("language")}
        symbols = []
        for c in chunks:
            s = c.get("symbol_name", "")
            if s and s not in symbols:
                symbols.append(s)

        # Enrich content with structural metadata so fallback_classification
        # has more signal (symbol names, kinds, languages).
        metadata_header = f"File: {file_source}\n"
        if languages:
            metadata_header += f"Languages: {', '.join(sorted(languages))}\n"
        if symbols:
            metadata_header += f"Symbols: {', '.join(symbols)}\n"
        enriched = metadata_header + "\n" + content[:12000]

        # Classify deterministically (no LLM).
        classification = fallback_classification(file_source, enriched)

        card = {
            "id": file_id,
            "label": file_source,
            "area": classification["area"],
            "cluster_semantic": classification["cluster_semantic"],
            "funzione_primaria": classification["funzione_primaria"],
            "espone_api": classification["espone_api"],
            "dipende_da": classification["dipende_da"],
            "simile_a": [],
            "tecnologie": classification["tecnologie"],
            "file_sorgente": file_source,
            "ultima_modifica": ultima_modifica,
            "source": "chunk-derived",
            "embedding_dims": EMBED_DIMS,
        }
        cards.append(card)

    # Write cards to SQLite.
    if cards:
        store.upsert_many(cards)

    # Write hash vectors to LanceDB (GPU-free, deterministic).
    if cards:
        vector_records = []
        for card in cards:
            text = f"{card['label']} {card['area']} {card['cluster_semantic']} {card['funzione_primaria']} {' '.join(card['tecnologie'])}"
            vector = embed_text(text, dims=EMBED_DIMS)
            vector_records.append(
                {
                    "id": card["id"],
                    "label": card["label"],
                    "area": card["area"],
                    "cluster_semantic": card["cluster_semantic"],
                    "vector": vector,
                }
            )
        LanceStore(lance_path).upsert(vector_records)

    return len(cards)
