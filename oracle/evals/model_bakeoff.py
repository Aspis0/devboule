from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import time
from typing import Any

from oracle.config import CHUNK_DB_PATH, LANCE_DB_PATH, SQLITE_PATH
from oracle.server.answerer import answer_from_context
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


DEFAULT_QUERIES = [
    {
        "id": "rnaseq_output_release",
        "query": "how rna-seq release the outputs after a successful run in the browser",
        "expected_files": [
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs",
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/outputs.mjs",
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/job_views.mjs",
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/runner_status.mjs",
        ],
        "required_terms": [
            "output_renders",
            "artifact_url",
            "manifest_url",
            "downloadRenderedArtifact",
            "Content-Disposition",
            "Results ready",
        ],
    },
    {
        "id": "rnaseq_scaleway_lifecycle",
        "query": "which files control rna-seq Scaleway instance lifecycle and terminal cleanup",
        "expected_files": [
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs",
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs",
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/instance_tier.mjs",
        ],
        "required_terms": [
            "cleanupScalewayInstanceAfterTerminal",
            "terminateScalewayInstance",
            "releaseScalewayInstanceSlot",
        ],
    },
    {
        "id": "rnaseq_browser_upload",
        "query": "which files implement RNA-seq browser upload sessions and safe upload completion",
        "expected_files": [
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs",
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/rna_upload_sessions.mjs",
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/job_views.mjs",
        ],
        "required_terms": [
            "createBrowserUploadSession",
            "completeBrowserUploadFile",
            "completeBrowserUploadSession",
            "getBrowserUploadSessionStatus",
        ],
    },
]

def main() -> int:
    parser = argparse.ArgumentParser(description="Compare Oracle retrieval plus answer models on fixed grounded questions.")
    parser.add_argument("--sqlite", default=str(SQLITE_PATH))
    parser.add_argument("--vectors", default=str(LANCE_DB_PATH))
    parser.add_argument("--chunks", default=str(CHUNK_DB_PATH))
    parser.add_argument("--limit", type=int, default=8)
    parser.add_argument("--ollama-model", action="append", default=[])
    parser.add_argument("--remote-provider", action="append", default=[], help="GDPR/ZDR provider to test: scaleway, infomaniak, or mistral.")
    parser.add_argument("--remote-model", action="append", default=[], help="Remote model to test. Repeat in the same order as --remote-provider.")
    parser.add_argument("--remote-base-url", action="append", default=[], help="Optional remote base URL. Required for Infomaniak product_id URLs.")
    parser.add_argument("--skip-local", action="store_true")
    parser.add_argument("--query-id", action="append", default=[])
    parser.add_argument("--api-key", default="")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    sqlite = SQLiteStore(args.sqlite)
    engine = QueryEngine(sqlite, LanceStore(args.vectors), LanceStore(args.chunks))
    query_ids = {item.strip() for item in args.query_id if item.strip()}
    queries = [item for item in DEFAULT_QUERIES if not query_ids or item["id"] in query_ids]
    if not queries:
        raise SystemExit("No matching query ids.")

    local_models = [item.strip() for item in args.ollama_model if item.strip()]
    if not local_models and not args.skip_local:
        local_models = ["qwen3.5:4b", "gemma4:e4b"]

    remote_specs = build_remote_specs(args)

    context_by_query = {}
    for query_spec in queries:
        chunks = engine.context(query_spec["query"], args.limit)
        context_by_query[query_spec["id"]] = chunks

    runs: list[dict[str, Any]] = []
    for model in ([] if args.skip_local else local_models):
        runs.append(run_model("ollama", model, queries, context_by_query))

    for spec in remote_specs:
        if spec["api_key"]:
            runs.append(
                run_model(
                    spec["provider"],
                    spec["model"],
                    queries,
                    context_by_query,
                    api_key=spec["api_key"],
                    base_url=spec["base_url"],
                )
            )
        else:
            runs.append(
                {
                    "provider": spec["provider"],
                    "model": spec["model"],
                    "error": "missing_api_key",
                    "results": [],
                    "score": 0,
                }
            )

    payload = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "limit": args.limit,
        "index": {
            "nodes": len(sqlite.all_nodes()),
            "chunk_files": sqlite.chunk_file_count(),
            "chunks": sqlite.chunk_count(),
            "chunk_vectors": LanceStore(args.chunks).count(),
        },
        "context": {
            item["id"]: summarize_context(item, context_by_query[item["id"]])
            for item in queries
        },
        "runs": runs,
    }

    out_path = Path(args.out) if args.out else Path("oracle-data") / f"model-bakeoff-{datetime.now().strftime('%Y%m%d-%H%M%S')}.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"out": str(out_path), "summary": summarize_runs(payload)}, ensure_ascii=False, indent=2))
    return 0


def run_model(
    provider: str,
    model: str,
    queries: list[dict],
    context_by_query: dict[str, list[dict]],
    api_key: str = "",
    base_url: str = "",
) -> dict:
    results = []
    start = time.perf_counter()
    for query_spec in queries:
        config = {"provider": provider, "model": model}
        if provider != "ollama":
            config.update(
                {
                    "api_key": api_key,
                    "base_url": base_url,
                }
            )
        item_start = time.perf_counter()
        answer = answer_from_context(query_spec["query"], context_by_query[query_spec["id"]], llm_config=config)
        elapsed_ms = int((time.perf_counter() - item_start) * 1000)
        quality = score_answer(query_spec, answer, context_by_query[query_spec["id"]])
        results.append(
            {
                "id": query_spec["id"],
                "query": query_spec["query"],
                "elapsed_ms": elapsed_ms,
                "answer": answer["answer"],
                "answer_source": answer.get("answer_source"),
                "fallback_reason": answer.get("fallback_reason"),
                "not_found": answer["not_found"],
                "citations": answer["citations"],
                "suggested_path": answer.get("suggested_path"),
                "quality": quality,
            }
        )
    return {
        "provider": provider,
        "model": model,
        "elapsed_ms": int((time.perf_counter() - start) * 1000),
        "score": sum(item["quality"]["score"] for item in results),
        "llm_ok": sum(1 for item in results if item["answer_source"] == "llm" and not item["not_found"]),
        "fallbacks": sum(1 for item in results if item["answer_source"] != "llm"),
        "not_found": sum(1 for item in results if item["not_found"]),
        "results": results,
    }


def summarize_context(query_spec: dict, chunks: list[dict]) -> dict:
    expected = set(query_spec["expected_files"])
    files = [chunk["file_source"] for chunk in chunks]
    return {
        "top_files": files[:8],
        "expected_hits": [path for path in files if path in expected],
        "expected_hit_count": sum(1 for path in files if path in expected),
        "required_term_hits": required_term_hits(query_spec["required_terms"], "\n".join(str(chunk.get("text") or "") for chunk in chunks)),
    }


def score_answer(query_spec: dict, answer: dict, chunks: list[dict]) -> dict:
    answer_text = str(answer.get("answer") or "")
    context_text = "\n".join(str(chunk.get("text") or "") for chunk in chunks)
    citation_files = [item.get("file_source") for item in answer.get("citations", [])]
    expected = set(query_spec["expected_files"])
    expected_context_hits = sum(1 for chunk in chunks if chunk.get("file_source") in expected)
    expected_citation_hits = sum(1 for path in citation_files if path in expected)
    term_hits_answer = required_term_hits(query_spec["required_terms"], answer_text)
    term_hits_context = required_term_hits(query_spec["required_terms"], context_text)
    score = 0
    score += min(expected_context_hits, 4) * 2
    score += min(expected_citation_hits, 3) * 3
    score += len(term_hits_answer)
    if answer.get("answer_source") == "llm" and not answer.get("not_found"):
        score += 4
    if answer.get("not_found"):
        score -= 8
    return {
        "score": score,
        "expected_context_hits": expected_context_hits,
        "expected_citation_hits": expected_citation_hits,
        "required_terms_in_answer": term_hits_answer,
        "required_terms_in_context": term_hits_context,
    }


def required_term_hits(terms: list[str], text: str) -> list[str]:
    lower = text.lower()
    return [term for term in terms if term.lower() in lower]


def summarize_runs(payload: dict) -> list[dict]:
    return [
        {
            "provider": run.get("provider"),
            "model": run.get("model"),
            "score": run.get("score"),
            "llm_ok": run.get("llm_ok"),
            "fallbacks": run.get("fallbacks"),
            "not_found": run.get("not_found"),
            "elapsed_ms": run.get("elapsed_ms"),
        }
        for run in payload["runs"]
    ]


def build_remote_specs(args) -> list[dict[str, str]]:
    providers = [item.strip().lower() for item in args.remote_provider if item.strip()]
    models = [item.strip() for item in args.remote_model if item.strip()]
    base_urls = [item.strip() for item in args.remote_base_url if item.strip()]
    if not providers and not models:
        return []
    if not providers or not models:
        raise SystemExit("--remote-provider and --remote-model must be supplied together.")
    if len(providers) == 1 and len(models) > 1:
        providers = providers * len(models)
    if len(providers) != len(models):
        raise SystemExit("--remote-provider and --remote-model counts must match, unless one provider is reused for many models.")

    specs = []
    for index, (provider, model) in enumerate(zip(providers, models)):
        if provider not in {"scaleway", "infomaniak", "mistral"}:
            raise SystemExit(f"Remote provider '{provider}' is not app-safe. Use scaleway, infomaniak, or mistral.")
        base_url = base_urls[index] if index < len(base_urls) else default_remote_base_url(provider)
        api_key = args.api_key.strip() or provider_api_key_from_env(provider)
        specs.append({"provider": provider, "model": model, "base_url": base_url, "api_key": api_key})
    return specs


def default_remote_base_url(provider: str) -> str:
    if provider == "scaleway":
        return "https://api.scaleway.ai/v1/chat/completions"
    if provider == "mistral":
        return "https://api.mistral.ai/v1/chat/completions"
    return ""


def provider_api_key_from_env(provider: str) -> str:
    if provider == "scaleway":
        return (
            os.getenv("SCW_SECRET_KEY", "").strip()
            or os.getenv("ASPIS_SCALEWAY_API_TOKEN", "").strip()
            or os.getenv("ORACLE_LLM_API_KEY", "").strip()
        )
    if provider == "infomaniak":
        return os.getenv("INFOMANIAK_API_TOKEN", "").strip() or os.getenv("ORACLE_LLM_API_KEY", "").strip()
    if provider == "mistral":
        return os.getenv("MISTRAL_API_KEY", "").strip() or os.getenv("ORACLE_LLM_API_KEY", "").strip()
    return ""


if __name__ == "__main__":
    raise SystemExit(main())
