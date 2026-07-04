import argparse
import json
import os
import sys
from typing import Any

from oracle.config import CHUNK_BATCH_CHARS, CHUNK_BATCH_FILES, CHUNK_DB_PATH, CHUNK_MANIFEST_PATH, CHUNK_MAX_GPU_TEMP_C, LANCE_DB_PATH, SQLITE_PATH
from oracle.ingestion.chunk_index import chunk_index_status, index_file_chunks, prune_excluded_chunks, sync_text_chunks
from oracle.verify_coverage import coverage
from oracle.verify_runtime import runtime_status
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


def main(argv: list[str] | None = None) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description="Architecture Oracle Phase 0/1 CLI")
    parser.add_argument("command", choices=["snapshot", "ask", "context", "node", "similar", "duplicates", "cluster", "coverage", "runtime", "index-chunks", "chunk-status", "sync-text-chunks", "prune-chunks"])
    parser.add_argument("--sqlite", default=str(SQLITE_PATH))
    parser.add_argument("--vectors", default=str(LANCE_DB_PATH))
    parser.add_argument("--chunks", default=str(CHUNK_DB_PATH))
    parser.add_argument("--manifest", default=str(CHUNK_MANIFEST_PATH))
    parser.add_argument("--root", default=os.getenv("ORACLE_INDEX_ROOT", "."))
    parser.add_argument("--query", default="")
    parser.add_argument("--node-id", default="")
    parser.add_argument("--cluster-id", default="")
    parser.add_argument("--limit", type=int, default=8)
    parser.add_argument("--batch-files", type=int, default=CHUNK_BATCH_FILES)
    # Tri-state: omitted -> None -> effective_chunk_batch_size derives the value
    # from the hardware-sized encode batch. ANY explicit value — including one
    # equal to the config default — is honored literally.
    parser.add_argument("--batch-chunks", type=int, default=None)
    parser.add_argument("--batch-chars", type=int, default=CHUNK_BATCH_CHARS)
    parser.add_argument("--max-batches", type=int, default=0)
    parser.add_argument("--min-free-gb", type=float, default=None)
    parser.add_argument("--min-free-ram-gb", type=float, default=None)
    parser.add_argument("--max-gpu-temp-c", type=int, default=CHUNK_MAX_GPU_TEMP_C)
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--allow-download", action="store_true")
    parser.add_argument("--fallback-hash", action="store_true")
    parser.add_argument("--progress", action="store_true")
    args = parser.parse_args(argv)

    if args.command == "index-chunks":
        if not args.allow_download:
            os.environ.setdefault("HF_HUB_OFFLINE", "1")
            os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
        # Resolve the free-RAM floor GPU-aware (mirrors the server run_once path):
        # an explicit --min-free-ram-gb/--min-free-gb wins; otherwise pick a low
        # floor on GPU/MPS (model lives in VRAM) and the conservative CPU floor
        # otherwise. idle=False here — the CLI is an explicit, foreground run.
        if args.min_free_ram_gb is not None:
            min_free_gb = args.min_free_ram_gb
        elif args.min_free_gb is not None:
            min_free_gb = args.min_free_gb
        else:
            from oracle.ingestion.embedder import embedding_device
            from oracle.server.index_jobs import resolve_min_free_gb

            min_free_gb = resolve_min_free_gb(embedding_device(), idle=False)
        print(json.dumps(
            index_file_chunks(
                args.root,
                args.sqlite,
                args.chunks,
                manifest_path=args.manifest,
                batch_files=args.batch_files,
                batch_chunks=args.batch_chunks,
                batch_chars=args.batch_chars,
                min_free_gb=min_free_gb,
                max_gpu_temp_c=args.max_gpu_temp_c,
                max_batches=args.max_batches or None,
                force=args.force,
                use_sentence_transformer=True,
                require_sentence_transformer=not args.fallback_hash,
                progress=args.progress,
            ),
            ensure_ascii=False,
        ))
        return 0

    if args.command == "sync-text-chunks":
        print(json.dumps(
            sync_text_chunks(args.root, args.sqlite, batch_files=args.batch_files, progress=args.progress),
            ensure_ascii=False,
        ))
        return 0

    if args.command == "prune-chunks":
        print(json.dumps(
            prune_excluded_chunks(
                args.root,
                args.sqlite,
                args.chunks,
                args.manifest,
                node_vector_path=args.vectors,
                progress=args.progress,
            ),
            ensure_ascii=False,
        ))
        return 0

    if args.command == "chunk-status":
        print(json.dumps(
            chunk_index_status(args.root, args.sqlite, args.chunks, args.manifest),
            ensure_ascii=False,
        ))
        return 0

    engine = QueryEngine(SQLiteStore(args.sqlite), LanceStore(args.vectors), LanceStore(args.chunks))
    payload = dispatch(engine, args)
    print(json.dumps(payload, ensure_ascii=False))
    return 0


def dispatch(engine: QueryEngine, args: argparse.Namespace) -> Any:
    if args.command == "snapshot":
        return snapshot_payload(engine)
    if args.command == "ask":
        return answer_payload(engine, args.query, args.limit)
    if args.command == "context":
        return {"query": args.query, "chunks": engine.context(args.query, args.limit)}
    if args.command == "node":
        return node_payload(engine, args.node_id)
    if args.command == "similar":
        return [result_payload(result) for result in engine.similar(args.node_id, args.limit)]
    if args.command == "duplicates":
        return duplicate_label_payload(engine)
    if args.command == "cluster":
        nodes = engine.cluster(args.cluster_id)
        return {
            "name": args.cluster_id,
            "node_count": len(nodes),
            "nodes": [result_payload(card_to_result(node)) for node in nodes[: max(1, args.limit)]],
        }
    if args.command == "coverage":
        return coverage(args.sqlite)
    if args.command == "runtime":
        return runtime_status(args.vectors)
    raise ValueError(f"Unsupported Oracle command: {args.command}")


def snapshot_payload(engine: QueryEngine) -> dict:
    health = engine.health()
    duplicates = duplicate_label_payload(engine)
    clusters = {node["cluster_semantic"] for node in engine.sqlite.all_nodes()}
    return {
        "status": health["status"],
        "source": "python-oracle",
        "phase": "phase1-python",
        "node_count": health["nodes"],
        "edge_count": 0,
        "cluster_count": len(clusters),
        "duplicate_labels": duplicates,
    }


def answer_payload(engine: QueryEngine, query: str, limit: int) -> dict:
    answer = engine.ask(query, limit)
    return {
        "mode": "python-oracle",
        "query": answer["query"],
        "summary": answer["summary"],
        "answer": answer.get("answer", answer["summary"]),
        "citations": answer.get("citations", []),
        "not_found": bool(answer.get("not_found", False)),
        "answer_source": answer.get("answer_source"),
        "fallback_reason": answer.get("fallback_reason"),
        "suggested_path": answer.get("suggested_path"),
        "results": [result_payload(result) for result in answer["results"]],
    }


def node_payload(engine: QueryEngine, node_id: str) -> dict:
    node = engine.node(node_id)
    return {
        "id": node["id"],
        "label": node["label"],
        "area": node["area"],
        "cluster_semantic": node["cluster_semantic"],
        "funzione_primaria": node["funzione_primaria"],
        "espone_api": node["espone_api"],
        "dipende_da": node["dipende_da"],
        "used_by": used_by(engine, node["id"]),
        "simile_a": [result["id"] for result in engine.similar(node["id"], 8)],
        "tecnologie": node["tecnologie"],
        "file_sorgente": node["file_sorgente"],
        "ultima_modifica": node["ultima_modifica"] or None,
        "source": node["source"],
        "embedding_dims": node["embedding_dims"],
    }


def duplicate_label_payload(engine: QueryEngine) -> list[dict]:
    by_id = {node["id"]: node for node in engine.sqlite.all_nodes()}
    groups = []
    for ids in engine.duplicates():
        label = by_id.get(ids[0], {}).get("label", ids[0])
        groups.append({"label": label, "node_ids": ids})
    return groups


def used_by(engine: QueryEngine, node_id: str) -> list[str]:
    users = []
    for node in engine.sqlite.all_nodes():
        if node_id in node["dipende_da"]:
            users.append(node["id"])
    return sorted(users)


def result_payload(result: dict) -> dict:
    return {
        "id": result["id"],
        "label": result["label"],
        "node_type": result.get("node_type", "file"),
        "cluster": parse_cluster(result.get("cluster", 0)),
        "score": float(result.get("score", 0.0)),
        "file_source": result.get("file_sorgente") or result.get("file_source") or result["id"],
        "function_primary": result.get("funzione_primaria") or result.get("function_primary") or "",
        "dependencies": result.get("dipende_da") or result.get("dependencies") or [],
        "chunk_id": result.get("chunk_id"),
        "chunk_index": result.get("chunk_index"),
        "start_char": result.get("start_char"),
        "end_char": result.get("end_char"),
        "chunk_preview": result.get("chunk_preview"),
    }


def card_to_result(card: dict) -> dict:
    return {
        "id": card["id"],
        "label": card["label"],
        "cluster": card["cluster_semantic"],
        "score": 1.0,
        "file_sorgente": card["file_sorgente"],
        "funzione_primaria": card["funzione_primaria"],
        "dipende_da": card["dipende_da"],
    }


def parse_cluster(value: Any) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return 0


if __name__ == "__main__":
    sys.exit(main())
