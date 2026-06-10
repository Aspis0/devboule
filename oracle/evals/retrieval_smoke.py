from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

from oracle.config import CHUNK_DB_PATH, LANCE_DB_PATH, SQLITE_PATH
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


DEFAULT_CASES = [
    {
        "id": "rnaseq_output_release",
        "query": "how rna-seq release the outputs after a successful run in the browser",
        "expected_files": ["aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs"],
        "required_terms": ["output_renders", "artifact_url", "manifest_url", "downloadRenderedArtifact"],
    },
    {
        "id": "rnaseq_browser_upload",
        "query": "which files implement RNA-seq browser upload sessions and safe upload completion",
        "expected_files": ["aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/rna_upload_sessions.mjs"],
        "required_terms": ["createBrowserUploadSession", "completeBrowserUploadFile", "completeBrowserUploadSession"],
    },
    {
        "id": "rnaseq_scaleway_lifecycle",
        "query": "which files control rna-seq Scaleway instance lifecycle and terminal cleanup",
        "expected_files": ["aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs"],
        "required_terms": ["cleanupScalewayInstanceAfterTerminal", "terminateScalewayInstance", "releaseScalewayInstanceSlot"],
    },
    {
        "id": "mcp_agent_oracle",
        "query": "how can CLI agents call Oracle and update project status through MCP",
        "expected_files": ["oracle/server/aspis_mcp.py"],
        "required_terms": ["oracle_ask", "oracle_context", "project_update_status"],
        "forbidden_top_files": ["oracle/ingestion/retrieval_text.py"],
    },
    {
        "id": "oracle_privacy_providers",
        "query": "which Oracle LLM providers are allowed for GDPR ZDR and where are they configured",
        "expected_files": ["src/components/views/OracleView.tsx", "src-tauri/src/backend/vault.rs"],
        "required_terms": ["scaleway", "infomaniak", "mistral"],
        "forbidden_top_files": ["oracle/ingestion/retrieval_text.py"],
    },
    {
        "id": "windows_hello_unlock",
        "query": "which files control Windows Hello camera PIN unlock loop",
        "expected_files": ["src-tauri/src/backend/auth.rs", "src/components/auth/LockedScreen.tsx"],
        "required_terms": ["Windows Hello", "unlock"],
        "forbidden_top_files": ["oracle/ingestion/retrieval_text.py"],
    },
    {
        "id": "cloudflare_secret_rotation",
        "query": "where is Cloudflare worker secret rotation implemented",
        "expected_files": ["src-tauri/src/backend/commands.rs", "src/components/dashboard/WorkersTable.tsx"],
        "required_terms": ["rotate_cloudflare_worker_secret", "put_cloudflare_worker_secret"],
    },
    {
        "id": "projects_mini_notion",
        "query": "which files implement the mini notion projects kanban and agent claims",
        "expected_files": ["src/components/views/ProjectsView.tsx", "oracle/server/aspis_mcp.py"],
        "required_terms": ["project_claim_task", "Projects"],
        "forbidden_top_files": ["oracle/ingestion/retrieval_text.py"],
    },
    {
        "id": "abstract_rnaseq_download",
        "query": "after a successful RNA sequencing analysis, how does the website know which output files the user can download",
        "expected_files": [
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs",
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/job_views.mjs",
        ],
        "required_terms": ["output_renders", "artifact_url", "manifest_url"],
        "forbidden_top_files": ["oracle/ingestion/retrieval_text.py", "oracle/server/query_engine.py"],
    },
    {
        "id": "abstract_agent_workflow",
        "query": "how do terminal agents know the current project task and mark it finished without editing the UI manually",
        "expected_files": ["oracle/server/aspis_mcp.py", "src/components/views/ProjectsView.tsx"],
        "required_terms": ["project_claim_task", "project_update_status"],
        "forbidden_top_files": ["oracle/ingestion/retrieval_text.py", "aspis-lab/cloudflare/Orasis/src/ai/prompt.ts"],
    },
    {
        "id": "abstract_oracle_privacy",
        "query": "where is the rule that only privacy safe AI providers can be used for Oracle answers",
        "expected_files": ["src/components/views/OracleView.tsx", "src-tauri/src/backend/vault.rs", "oracle/server/answerer.py"],
        "required_terms": ["zdr", "gdpr", "scaleway", "infomaniak", "mistral"],
        "forbidden_top_files": ["oracle/ingestion/retrieval_text.py", "oracle/server/aspis_mcp.py"],
    },
    {
        "id": "abstract_scaleway_paid_cleanup",
        "query": "where do we stop paid Scaleway compute resources after a job or terminal session is done",
        "expected_files": [
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs",
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs",
        ],
        "required_terms": ["terminateScalewayInstance", "releaseScalewayInstanceSlot"],
        "forbidden_top_files": ["oracle/server/query_engine.py", "oracle/ingestion/retrieval_text.py"],
    },
    {
        "id": "abstract_windows_camera_loop",
        "query": "which code stops the Windows Hello webcam unlock from opening repeatedly",
        "expected_files": ["src-tauri/src/backend/auth.rs", "src/components/auth/LockedScreen.tsx"],
        "required_terms": ["Windows Hello", "unlock"],
        "forbidden_top_files": ["oracle/bootstrap/ingest_legacy_graph.py", "src-tauri/src/graph/oracle.rs"],
    },
]


def main() -> int:
    parser = argparse.ArgumentParser(description="Oracle retrieval-only smoke tests. Does not call Ollama or remote LLMs.")
    parser.add_argument("--sqlite", default=str(SQLITE_PATH))
    parser.add_argument("--vectors", default=str(LANCE_DB_PATH))
    parser.add_argument("--chunks", default=str(CHUNK_DB_PATH))
    parser.add_argument("--limit", type=int, default=8)
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    engine = QueryEngine(SQLiteStore(args.sqlite), LanceStore(args.vectors), LanceStore(args.chunks))
    results = [run_case(engine, case, args.limit) for case in DEFAULT_CASES]
    payload = {
        "status": "pass" if all(item["pass"] for item in results) else "fail",
        "limit": args.limit,
        "index": {
            "files": engine.sqlite.chunk_file_count(),
            "chunks": engine.sqlite.chunk_count(),
            "vectors": engine.chunk_vectors.count() if engine.chunk_vectors else 0,
        },
        "results": results,
    }
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(payload, ensure_ascii=False, indent=2))
    return 0 if payload["status"] == "pass" else 1


def run_case(engine: QueryEngine, case: dict, limit: int) -> dict:
    chunks = engine.context(case["query"], limit)
    files = [chunk["file_source"] for chunk in chunks]
    text = "\n".join(str(chunk.get("text") or "") for chunk in chunks)
    expected_hits = [path for path in case["expected_files"] if path in files]
    term_hits = [term for term in case["required_terms"] if term.lower() in text.lower()]
    forbidden_top_hits = [
        path
        for path in case.get("forbidden_top_files", [])
        if path in files[: min(3, len(files))]
    ]
    answer = run_bounded_answer(engine, case["query"], limit)
    answer_files = answer_cited_files(answer)
    answer_expected_hits = [path for path in case["expected_files"] if path in answer_files]
    answer_passed = (
        not bool(answer.get("not_found"))
        and bool(answer.get("answer") or answer.get("summary"))
        and bool(answer_files)
        and bool(answer_expected_hits)
    )
    passed = (
        bool(expected_hits)
        and len(term_hits) >= max(1, min(2, len(case["required_terms"])))
        and not forbidden_top_hits
        and answer_passed
    )
    return {
        "id": case["id"],
        "pass": passed,
        "query": case["query"],
        "expected_hits": expected_hits,
        "required_term_hits": term_hits,
        "forbidden_top_hits": forbidden_top_hits,
        "top_files": files[:limit],
        "answer_pass": answer_passed,
        "answer_source": answer.get("answer_source"),
        "answer_expected_hits": answer_expected_hits,
        "answer_files": answer_files[:limit],
        "answer_preview": str(answer.get("answer") or answer.get("summary") or "")[:240],
    }


def run_bounded_answer(engine: QueryEngine, query: str, limit: int) -> dict:
    previous = os.environ.get("ORACLE_ASK_DISABLE_LLM")
    os.environ["ORACLE_ASK_DISABLE_LLM"] = "1"
    try:
        return engine.ask(query, limit)
    finally:
        if previous is None:
            os.environ.pop("ORACLE_ASK_DISABLE_LLM", None)
        else:
            os.environ["ORACLE_ASK_DISABLE_LLM"] = previous


def answer_cited_files(answer: dict) -> list[str]:
    files = []
    for citation in answer.get("citations") or []:
        file_source = citation.get("file_source") or citation.get("fileSource")
        if file_source:
            files.append(file_source)
    for result in answer.get("results") or []:
        file_source = result.get("file_source") or result.get("fileSource")
        if file_source:
            files.append(file_source)
    seen = set()
    unique = []
    for file_source in files:
        if file_source not in seen:
            unique.append(file_source)
            seen.add(file_source)
    return unique


if __name__ == "__main__":
    raise SystemExit(main())
