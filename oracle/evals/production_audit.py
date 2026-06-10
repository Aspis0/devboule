from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

from oracle.config import CHUNK_DB_PATH, CHUNK_MANIFEST_PATH, LANCE_DB_PATH, SQLITE_PATH
from oracle.ingestion.chunk_index import chunk_index_status
from oracle.server.answerer import answer_from_context, validate_remote_llm_config
from oracle.server.aspis_mcp import read_project_file, oracle_llm_config_from_app_vault
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore
from oracle.evals.retrieval_smoke import DEFAULT_CASES, run_case as run_retrieval_case
from oracle.verify_runtime import runtime_status


REQUIRED_PROJECT_ID = "oracle-production-readiness"
REQUIRED_AGENT_IDS = {"prod-architect", "prod-code", "prod-verifier"}


def main() -> int:
    parser = argparse.ArgumentParser(description="Aspis Oracle production readiness audit.")
    parser.add_argument("--root", default=".", help="Aspis Management root")
    parser.add_argument("--project-root", default=str(Path.home() / "Desktop" / "aspis bio"))
    parser.add_argument("--sqlite", default=str(SQLITE_PATH))
    parser.add_argument("--vectors", default=str(LANCE_DB_PATH))
    parser.add_argument("--chunks", default=str(CHUNK_DB_PATH))
    parser.add_argument("--manifest", default=str(CHUNK_MANIFEST_PATH))
    parser.add_argument("--skip-mcp-smoke", action="store_true")
    parser.add_argument("--live-remote", action="store_true", help="Call the configured remote Oracle provider.")
    parser.add_argument("--strict-remote", action="store_true", help="Fail if primary+fallback remote providers are not configured.")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    root = Path(args.root).resolve()
    project_root = Path(args.project_root).resolve()
    engine = QueryEngine(SQLiteStore(args.sqlite), LanceStore(args.vectors), LanceStore(args.chunks))
    checks = [
        audit_index("management_index", root, args),
        audit_index("project_index", project_root, args),
        audit_app_surface(root),
        audit_project(root),
        audit_retrieval_suite(engine),
        audit_runtime(args.vectors),
        audit_bounded_answer(engine),
        audit_provider_config(strict_remote=args.strict_remote),
    ]
    if args.live_remote:
        checks.extend(audit_live_remote_answers(strict_remote=args.strict_remote))
    if not args.skip_mcp_smoke:
        checks.append(audit_mcp_smoke(root, project_root))

    payload = {
        "status": "pass" if all(check["pass"] for check in checks) else "fail",
        "root": str(root),
        "project_root": str(project_root),
        "checks": checks,
    }
    if args.out:
        out = Path(args.out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(payload, ensure_ascii=False, indent=2))
    return 0 if payload["status"] == "pass" else 1


def audit_index(name: str, root: Path, args: argparse.Namespace) -> dict[str, Any]:
    status = chunk_index_status(root, args.sqlite, args.chunks, args.manifest)
    passed = (
        status["expected_files"] > 0
        and status["indexed_files"] == status["expected_files"]
        and status["pending_files"] == 0
        and status["stale_files"] == 0
        and status["sqlite_chunks"] == status["vector_records"]
    )
    return {
        "name": name,
        "pass": passed,
        "root": status["root"],
        "expected_files": status["expected_files"],
        "indexed_files": status["indexed_files"],
        "pending_files": status["pending_files"],
        "stale_files": status["stale_files"],
        "sqlite_chunks": status["sqlite_chunks"],
        "vector_records": status["vector_records"],
        "first_pending": status["first_pending"],
        "first_stale": status["first_stale"],
    }


def audit_app_surface(root: Path) -> dict[str, Any]:
    files = {
        "app": root / "src" / "App.tsx",
        "projects": root / "src" / "components" / "views" / "ProjectsView.tsx",
        "agents": root / "src" / "components" / "views" / "AgentsView.tsx",
        "oracle": root / "src" / "components" / "views" / "OracleView.tsx",
        "agent_claims": root / "src" / "utils" / "agentClaims.ts",
        "lib": root / "src-tauri" / "src" / "lib.rs",
        "projects_backend": root / "src-tauri" / "src" / "backend" / "projects.rs",
    }
    texts = {}
    missing_files = []
    for name, path in files.items():
        if not path.exists():
            missing_files.append(str(path))
            texts[name] = ""
        else:
            texts[name] = path.read_text(encoding="utf-8", errors="replace")

    source_requirements = {
        "projects_route": '"projects"' in texts["app"] and "ProjectsView" in texts["app"],
        "oracle_route": '"oracle"' in texts["app"] and "OracleView" in texts["app"],
        "agents_route": '"agents"' in texts["app"] and "AgentsView" in texts["app"],
        "projects_surface": "Project workspace" in texts["projects"] and "Launch agents" in texts["projects"],
        "task_sessions_visible": "sessionsByTask" in texts["projects"] and "currentTaskId" in texts["projects"],
        "project_stage_active_priority": "project.taskCounts.wip > 0" in texts["projects"] and "currentProjectSessions" in texts["projects"],
        "project_stage_launching_split": "launching" in texts["projects"] and "launch_pending" in texts["projects"],
        "leaseless_claims_expire": "ACTIVE_SESSION_WINDOW_MS" in texts["agent_claims"] and "updatedAt" in texts["agent_claims"],
        "agents_mcp_config": "mcpClientConfig" in texts["agents"] and "Copy MCP config" in texts["agents"],
        "agent_launch_tokens": "prepare_project_agent_prompt" in texts["projects"] and "launch_token" in texts["projects_backend"],
        "agent_session_tokens": "session_token" in texts["projects_backend"],
        "launcher_no_management_add_dir": "--add-dir" not in texts["projects_backend"],
        "tauri_launcher_mcp_attached": "mcp_servers.aspis-management" in texts["projects_backend"] and "--mcp-config" in texts["projects_backend"],
        "oracle_dense_index_surface": "Index now" in texts["oracle"] and "Dense index ready" in texts["oracle"],
        "oracle_fallback_surface": "Fallback used" in texts["oracle"] and "fallbackFromProvider" in texts["oracle"],
        "tauri_projects_commands": all(
            token in texts["lib"]
            for token in [
                "backend::projects::list_projects",
                "backend::projects::get_project",
                "backend::projects::launch_project_agent_terminal",
                "backend::projects::prepare_project_agent_prompt",
            ]
        ),
        "tauri_oracle_commands": all(
            token in texts["lib"]
            for token in ["graph::commands::get_oracle_snapshot", "graph::commands::ask_oracle"]
        ),
    }

    dist_dir = root / "dist"
    built_js = "\n".join(
        path.read_text(encoding="utf-8", errors="replace")
        for path in (dist_dir / "assets").glob("*.js")
    ) if (dist_dir / "assets").exists() else ""
    dist_assets = list((dist_dir / "assets").glob("*")) if (dist_dir / "assets").exists() else []
    newest_source_mtime = max(
        (path.stat().st_mtime for path in files.values() if path.exists()),
        default=0,
    )
    newest_dist_mtime = max(
        ([path.stat().st_mtime for path in dist_assets if path.is_file()] + [(dist_dir / "index.html").stat().st_mtime] if (dist_dir / "index.html").exists() else []),
        default=0,
    )
    bundle_requirements = {
        "dist_exists": (dist_dir / "index.html").exists() and bool(built_js),
        "dist_fresh": newest_dist_mtime >= newest_source_mtime,
        "bundle_projects": "Project workspace" in built_js,
        "bundle_agents": "Agent control room" in built_js and "Copy MCP config" in built_js,
        "bundle_oracle": "Oracle ready" in built_js and "Dense index ready" in built_js,
    }
    failed = [
        name
        for name, passed in {**source_requirements, **bundle_requirements}.items()
        if not passed
    ]
    return {
        "name": "app_projects_oracle_surface",
        "pass": not missing_files and not failed,
        "missing_files": missing_files,
        "failed": failed,
        "source": source_requirements,
        "bundle": bundle_requirements,
        "newest_source_mtime": newest_source_mtime,
        "newest_dist_mtime": newest_dist_mtime,
    }


def audit_project(root: Path) -> dict[str, Any]:
    project_path = root / "projects" / f"{REQUIRED_PROJECT_ID}.md"
    agents_path = root / "projects" / ".aspis-agents.json"
    project: dict[str, Any] | None = None
    project_error = None
    if project_path.exists():
        try:
            project = read_project_file(project_path)
        except Exception as exc:
            project_error = str(exc)
    agents = json.loads(agents_path.read_text(encoding="utf-8")) if agents_path.exists() else {}
    sessions = {item.get("agentId"): item for item in agents.get("sessions", [])}
    claims = agents.get("claims", [])
    tasks = project.get("state", {}).get("tasks", []) if project else []
    statuses = {item.get("id"): item.get("status") for item in tasks}
    project_claims = [item for item in claims if item.get("projectId") == REQUIRED_PROJECT_ID]
    claim_drift = [
        {
            "agent_id": item.get("agentId"),
            "task_id": item.get("taskId"),
            "claim_status": item.get("status"),
            "task_status": statuses.get(item.get("taskId")),
        }
        for item in project_claims
        if item.get("taskId") not in statuses or item.get("status") != statuses.get(item.get("taskId"))
    ]
    missing_agents = sorted(REQUIRED_AGENT_IDS - set(sessions))
    passed = (
        project_path.exists()
        and agents_path.exists()
        and project_error is None
        and not missing_agents
        and not claim_drift
        and any(status == "done" for status in statuses.values())
        and any(status == "review" for status in statuses.values())
        and any(item.get("status") == "review" for item in project_claims)
    )
    return {
        "name": "projects_kanban_agent_state",
        "pass": passed,
        "project": str(project_path),
        "project_exists": project_path.exists(),
        "project_error": project_error,
        "agent_state_exists": agents_path.exists(),
        "missing_agents": missing_agents,
        "task_statuses": statuses,
        "claim_drift": claim_drift,
        "claim_count": len(claims),
        "project_claim_count": len(project_claims),
        "event_count": len(agents.get("events", [])),
    }


def audit_retrieval_suite(engine: QueryEngine) -> dict[str, Any]:
    results = [run_retrieval_case(engine, case, 8) for case in DEFAULT_CASES]
    failed = [item["id"] for item in results if not item["pass"]]
    return {
        "name": "oracle_retrieval_suite",
        "pass": not failed,
        "case_count": len(results),
        "failed": failed,
        "top_files": {
            item["id"]: item.get("top_files", [])[:3]
            for item in results
        },
    }


def audit_runtime(vector_path: str) -> dict[str, Any]:
    status = runtime_status(vector_path)
    ollama = status.get("ollama", {})
    vector = status.get("vector_store", status.get("vector", {}))
    chunks = status.get("chunk_store", {})
    passed = (
        bool(vector.get("ready"))
        and bool(chunks.get("ready"))
        and str(ollama.get("server") or ollama.get("status") or "") == "ready"
        and bool(ollama.get("model_available", True))
        and str(ollama.get("model") or "").startswith("qwen3.5")
    )
    return {
        "name": "oracle_runtime_local_qwen",
        "pass": passed,
        "vector_ready": vector.get("ready"),
        "vector_backend": vector.get("backend"),
        "vector_records": vector.get("records"),
        "chunk_ready": chunks.get("ready"),
        "chunk_records": chunks.get("records"),
        "chunk_vector_records": chunks.get("vector_records"),
        "ollama_status": ollama.get("server") or ollama.get("status"),
        "ollama_model": ollama.get("model"),
        "ollama_model_available": ollama.get("model_available"),
    }


def audit_bounded_answer(engine: QueryEngine) -> dict[str, Any]:
    old_value = os.environ.get("ORACLE_ASK_DISABLE_LLM")
    os.environ["ORACLE_ASK_DISABLE_LLM"] = "1"
    try:
        answer = engine.ask("How do terminal agents update project task status through MCP?", 5)
    finally:
        if old_value is None:
            os.environ.pop("ORACLE_ASK_DISABLE_LLM", None)
        else:
            os.environ["ORACLE_ASK_DISABLE_LLM"] = old_value
    passed = (
        not answer.get("not_found")
        and bool(answer.get("citations"))
        and "project_update_status" in str(answer.get("answer") or "")
        and bool(answer.get("results"))
    )
    return {
        "name": "oracle_bounded_grounded_answer",
        "pass": passed,
        "answer_source": answer.get("answer_source"),
        "fallback_reason": answer.get("fallback_reason"),
        "citation_files": [item.get("file_source") for item in answer.get("citations", [])],
        "result_files": [item.get("file_source") for item in answer.get("results", [])[:5]],
    }


def audit_provider_config(strict_remote: bool) -> dict[str, Any]:
    config = oracle_llm_config_from_app_vault()
    if not config:
        return {
            "name": "oracle_provider_config",
            "pass": not strict_remote,
            "configured": False,
            "strict_remote": strict_remote,
            "reason": "No Oracle LLM settings found in app vault.",
        }
    primary = safe_provider_summary(config)
    primary_valid = validate_provider(config)
    return {
        "name": "oracle_provider_config",
        "pass": primary_valid,
        "strict_remote": strict_remote,
        "primary": primary,
        "primary_valid": primary_valid,
    }


def validate_provider(config: dict[str, Any]) -> bool:
    if config.get("provider") == "ollama":
        return True
    try:
        validate_remote_llm_config(config)
        return True
    except Exception:
        return False


def safe_provider_summary(config: dict[str, Any]) -> dict[str, Any]:
    return {
        "provider": config.get("provider"),
        "model": config.get("model"),
        "base_url": config.get("base_url"),
        "api_key_present": bool(config.get("api_key")),
    }


def audit_live_remote_answers(strict_remote: bool = False) -> list[dict[str, Any]]:
    config = oracle_llm_config_from_app_vault()
    if not config or config.get("provider") == "ollama":
        return [{
            "name": "oracle_live_remote_answer",
            "pass": False,
            "reason": "Remote Oracle provider is not configured as primary.",
        }]
    chunk = {
        "chunk_id": "oracle/server/aspis_mcp.py#live-audit",
        "file_source": "oracle/server/aspis_mcp.py",
        "chunk_index": 0,
        "start_char": 0,
        "end_char": 220,
        "retrieval": "audit",
        "score": 99.0,
        "text": "Agents call project_claim_task and project_update_status to update task status through MCP.",
    }
    answer = answer_from_context("How do agents update project task status through MCP?", [chunk], llm_config=config)
    primary_passed = (
        answer.get("answer_source") == "llm"
        and not answer.get("not_found")
        and answer.get("llm_provider") == config.get("provider")
    )
    primary_check = {
        "name": "oracle_live_remote_primary",
        "pass": primary_passed or not strict_remote,
        "live_pass": primary_passed,
        "required": strict_remote,
        "provider": config.get("provider"),
        "model": config.get("model"),
        "answered_by": answer.get("llm_provider"),
        "answered_model": answer.get("llm_model"),
        "failure_reason": answer.get("fallback_reason"),
        "answer_source": answer.get("answer_source"),
        "citation_count": len(answer.get("citations", [])),
    }

    aggregate = {
        "name": "oracle_live_remote_answer",
        "pass": primary_passed or not strict_remote,
        "strict_remote": strict_remote,
        "provider": answer.get("llm_provider"),
        "model": answer.get("llm_model"),
        "answer_source": answer.get("answer_source"),
        "fallback_reason": answer.get("fallback_reason"),
        "citation_count": len(answer.get("citations", [])),
        "primary_live_pass": primary_passed,
    }
    return [primary_check, aggregate]


def audit_mcp_smoke(root: Path, project_root: Path) -> dict[str, Any]:
    command = [
        sys.executable,
        "-m",
        "oracle.evals.mcp_production_smoke",
        "--root",
        str(root),
        "--project-root",
        str(project_root),
    ]
    completed = subprocess.run(
        command,
        cwd=root,
        capture_output=True,
        text=True,
        timeout=300,
        encoding="utf-8",
        errors="replace",
    )
    payload = parse_last_json(completed.stdout)
    return {
        "name": "mcp_stdio_agents",
        "pass": completed.returncode == 0 and payload.get("status") == "pass",
        "returncode": completed.returncode,
        "tools_checked": payload.get("tools_checked", []),
        "provider_reads": payload.get("provider_reads"),
        "stderr_tail": completed.stderr[-1200:],
    }


def parse_last_json(text: str) -> dict[str, Any]:
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        pass
    start = text.rfind("{")
    if start < 0:
        return {}
    try:
        return json.loads(text[start:])
    except json.JSONDecodeError:
        return {}


if __name__ == "__main__":
    raise SystemExit(main())
