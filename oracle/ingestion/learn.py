from pathlib import Path

from oracle.config import EMBED_DIMS, LANCE_DB_PATH, SQLITE_PATH
from oracle.ingestion.classifier import classify_file
from oracle.ingestion.embedder import build_text_for_card, embed_texts
from oracle.ingestion.parser import parse_file
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


def learn_files(
    paths: list[str | Path],
    project_root: str | Path = ".",
    sqlite_path: str | Path = SQLITE_PATH,
    vector_path: str | Path = LANCE_DB_PATH,
    use_sentence_transformer: bool = True,
    use_ollama_classifier: bool = True,
) -> int:
    root = Path(project_root).resolve()
    parsed = []
    seen = set()
    for path in paths:
        item = parse_file(path, root)
        if not item or item["id"] in seen:
            continue
        seen.add(item["id"])
        parsed.append(item)

    if not parsed:
        return 0

    cards = []
    texts = []
    for item in parsed:
        classification = classify_file(
            item["id"],
            item["content"],
            use_ollama=use_ollama_classifier,
        )
        card = {
            "id": item["id"],
            "label": item["label"],
            "area": classification["area"],
            "cluster_semantic": classification["cluster_semantic"],
            "funzione_primaria": classification["funzione_primaria"],
            "espone_api": classification["espone_api"],
            "dipende_da": classification["dipende_da"],
            "simile_a": [],
            "tecnologie": classification["tecnologie"],
            "file_sorgente": item["file_sorgente"],
            "ultima_modifica": item["ultima_modifica"],
            "source": "oracle",
            "embedding_dims": EMBED_DIMS,
        }
        cards.append(card)
        texts.append(build_text_for_card(card, item["content"]))

    vectors = embed_texts(texts, use_sentence_transformer=use_sentence_transformer)
    SQLiteStore(sqlite_path).upsert_many(cards)
    LanceStore(vector_path).upsert([{**card, "vector": vector} for card, vector in zip(cards, vectors)])
    return len(cards)
