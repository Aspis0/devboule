import json
import re
from pathlib import Path
from typing import Iterable

from oracle.config import EMBED_DIMS, LANCE_DB_PATH, LEGACY_GRAPH_JSON, SQLITE_PATH
from oracle.ingestion.embedder import embed_texts
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


API_RE = re.compile(r"\b(GET|POST|PUT|PATCH|DELETE)\s+(/[A-Za-z0-9_./:*-]*)")


def build_text_for_embedding(node: dict) -> str:
    metadata = node.get("metadata") or {}
    parts = [
        node.get("label", ""),
        node.get("description", ""),
        metadata.get("docstring", ""),
        " ".join(node.get("tags", []) or []),
        " ".join(metadata.get("tags", []) or []),
        " ".join(node.get("imports", []) or []),
        " ".join(metadata.get("dependencies", []) or []),
    ]
    return " ".join(p for p in parts if p).strip()


def ingest(
    graph_path: Path | str = LEGACY_GRAPH_JSON,
    sqlite_path: Path | str = SQLITE_PATH,
    vector_path: Path | str = LANCE_DB_PATH,
    use_sentence_transformer: bool = True,
) -> int:
    graph_path = resolve_graph_path(Path(graph_path))
    data = json.loads(graph_path.read_text(encoding="utf-8"))
    nodes = data.get("nodes", [])

    records = []
    vectors = _build_embeddings(nodes, use_sentence_transformer)
    for node, vector in zip(nodes, vectors):
        card = node_to_card(node)
        records.append({**card, "vector": vector})

    sqlite = SQLiteStore(sqlite_path)
    sqlite.replace_all([{k: v for k, v in record.items() if k != "vector"} for record in records])

    lance = LanceStore(vector_path)
    lance.replace_all(records)
    return len(records)


def resolve_graph_path(graph_path: Path) -> Path:
    if graph_path.exists():
        return graph_path
    if graph_path == LEGACY_GRAPH_JSON and Path("graph.json").exists():
        return Path("graph.json")
    raise FileNotFoundError(f"Legacy graph export not found: {graph_path}")


def _build_embeddings(nodes: Iterable[dict], use_sentence_transformer: bool) -> list[list[float]]:
    # Route through the central guarded embedder so ORACLE_REQUIRE_REAL_EMBEDDER
    # is honored here too: a real-model failure RAISES instead of silently
    # writing hash vectors. The hash mock is reachable only when the production
    # hard-switch is unset (tests/recovery).
    texts = [build_text_for_embedding(node) for node in nodes]
    return embed_texts(texts, use_sentence_transformer=use_sentence_transformer)


def node_to_card(node: dict) -> dict:
    metadata = node.get("metadata") or {}
    doc = node.get("description") or metadata.get("docstring") or ""
    dependencies = node.get("imports") or metadata.get("dependencies") or []
    tags = [*(node.get("tags") or []), *(metadata.get("tags") or [])]
    cluster = str(node.get("cluster") or metadata.get("cluster") or "unknown")
    area = str(node.get("community") or metadata.get("area") or infer_area(node, doc, tags))

    return {
        "id": node["id"],
        "label": node.get("label") or node["id"],
        "area": area,
        "cluster_semantic": cluster if cluster.startswith("cluster-") else cluster,
        "funzione_primaria": doc,
        "espone_api": extract_api_hints(doc),
        "dipende_da": list(dependencies),
        "simile_a": [],
        "tecnologie": infer_technologies(node, doc, tags),
        "file_sorgente": node.get("file") or node["id"],
        "ultima_modifica": node.get("modified") or metadata.get("modified") or "",
        "source": "legacy-graph",
        "embedding_dims": EMBED_DIMS,
    }


def extract_api_hints(text: str) -> list[str]:
    return [f"{method} {path}" for method, path in API_RE.findall(text)]


def infer_area(node: dict, doc: str, tags: list[str]) -> str:
    text = " ".join([node.get("id", ""), node.get("label", ""), doc, " ".join(tags)]).lower()
    if "scaleway" in text or "serverless" in text or "gpu" in text:
        return "Scaleway"
    if "android" in text:
        return "CF-Android" if "worker" in text or "cloudflare" in text else "App"
    if "ios" in text:
        return "CF-iOS" if "worker" in text or "cloudflare" in text else "App"
    if "browser" in text:
        return "CF-Browser"
    if "cloudflare" in text or "worker" in text:
        return "Cloudflare"
    return "Codebase"


def infer_technologies(node: dict, doc: str, tags: list[str]) -> list[str]:
    text = " ".join([node.get("id", ""), node.get("label", ""), doc, " ".join(tags)]).lower()
    technologies = []
    for needle, label in [
        ("cloudflare", "Cloudflare Workers"),
        ("worker", "Cloudflare Workers"),
        ("scaleway", "Scaleway"),
        ("serverless", "Serverless"),
        ("gpu", "GPU"),
        ("jwt", "JWT"),
        ("kv", "KV"),
        ("react", "React"),
        ("tauri", "Tauri"),
        ("rust", "Rust"),
        ("typescript", "TypeScript"),
    ]:
        if needle in text and label not in technologies:
            technologies.append(label)
    return technologies


if __name__ == "__main__":
    print(f"Imported {ingest()} nodes from legacy graph export.")
