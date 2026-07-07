from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


PROJECT_ID = "oracle-production-readiness"


def write_smoke_project(projects: Path, project_root: Path, project_id: str) -> Path:
    projects.mkdir(parents=True, exist_ok=True)
    path = projects / f"{project_id}.md"
    content = {
        "version": 1,
        "tasks": [
            {
                "id": "T1",
                "title": "Prove MCP agent Kanban lifecycle",
                "status": "todo",
                "priority": "high",
                "assignee": None,
                "due": None,
                "linkedResources": [],
                "updatedAt": "2026-05-29T00:00:00Z",
            },
            {
                "id": "T2",
                "title": "Leave Oracle answer quality ready for human review",
                "status": "todo",
                "priority": "high",
                "assignee": None,
                "due": None,
                "linkedResources": [],
                "updatedAt": "2026-05-29T00:00:00Z",
            },
        ],
        "notes": [],
    }
    path.write_text(
        "\n".join(
            [
                "---",
                f"id: {project_id}",
                "title: Oracle Production Readiness",
                "status: active",
                "updated_at: 2026-05-29T00:00:00Z",
                f"root_path: {json.dumps(str(project_root))}",
                "---",
                "",
                "# Objectives",
                "- Prove real stdio MCP clients drive the Projects Kanban state machine.",
                "- Keep a visible production-readiness project for the Windows app.",
                "",
                "```aspis-project",
                json.dumps(content, indent=2),
                "```",
                "",
                "# Notes",
                "",
            ]
        ),
        encoding="utf-8",
    )
    return path


def hash_launch_token(token: str) -> str:
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def seed_launch_pending_agents(
    projects: Path,
    project_id: str,
    project_title: str,
    agents: dict[str, tuple[str, str, str | None]],
) -> None:
    now = "2099-01-01T00:00:00+00:00"
    state_path = projects / ".aspis-agents.json"
    if state_path.exists():
        try:
            state = json.loads(state_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            state = {}
    else:
        state = {}
    state.setdefault("version", 1)
    state["updatedAt"] = now
    state.setdefault("sessions", [])
    state.setdefault("claims", [])
    state.setdefault("events", [])
    state["sessions"] = [
        item for item in state["sessions"] if item.get("agentId") not in agents
    ]
    for agent_id, (role, token, task_id) in agents.items():
        state["sessions"].append(
            {
                "agentId": agent_id,
                "role": role,
                "model": None,
                "status": "launch_pending",
                "message": f"Terminal launched for {project_title}.",
                "currentProjectId": project_id,
                "currentTaskId": task_id,
                "firstSeenAt": now,
                "lastSeenAt": now,
                "launchTokenHash": hash_launch_token(token),
                "launchTokenIssuedAt": now,
            }
        )
    state_path.write_text(json.dumps(state, indent=2), encoding="utf-8")


def text_payload(result: Any) -> str:
    return "\n".join(getattr(item, "text", "") for item in result.content).strip()


def json_payload(result: Any) -> dict[str, Any]:
    text = text_payload(result)
    return json.loads(text) if text else {}


async def call(
    session: ClientSession,
    name: str,
    args: dict[str, Any] | None = None,
    expect_error: bool = False,
    timeout: int = 60,
) -> Any:
    print(f"mcp-smoke call {name}", file=sys.stderr, flush=True)
    result = await asyncio.wait_for(
        session.call_tool(name, args or {}), timeout=timeout
    )
    is_error = bool(getattr(result, "isError", False))
    if expect_error:
        if not is_error:
            raise AssertionError(
                f"{name} unexpectedly succeeded: {text_payload(result)}"
            )
        return text_payload(result)
    if is_error:
        raise AssertionError(f"{name} failed: {text_payload(result)}")
    return json_payload(result)


async def call_status(
    session: ClientSession,
    name: str,
    args: dict[str, Any] | None = None,
    timeout: int = 60,
) -> dict[str, Any]:
    print(f"mcp-smoke call {name}", file=sys.stderr, flush=True)
    try:
        result = await asyncio.wait_for(
            session.call_tool(name, args or {}), timeout=timeout
        )
    except Exception as exc:
        return {"pass": False, "error": f"{type(exc).__name__}: {str(exc)[:500]}"}
    text = text_payload(result)
    if bool(getattr(result, "isError", False)):
        return {"pass": False, "error": text[:500]}
    try:
        payload = json.loads(text) if text else {}
    except json.JSONDecodeError:
        return {
            "pass": False,
            "error": "Tool returned non-JSON payload.",
            "raw": text[:500],
        }
    return {"pass": True, "payload": payload}


async def run(
    root: Path,
    project_root: Path,
    keep_project: bool = False,
    live_providers: bool = False,
    project_id_override: str | None = None,
) -> dict[str, Any]:
    project_id = project_id_override or (
        PROJECT_ID
        if keep_project
        else f"{PROJECT_ID}-smoke-{os.getpid()}-{time.time_ns()}"
    )
    agent_suffix = "" if keep_project and not project_id_override else f"-{project_id}"
    architect_id = f"prod-architect{agent_suffix}"
    coder_id = f"prod-code{agent_suffix}"
    verifier_id = f"prod-verifier{agent_suffix}"
    architect_token = f"architect-token-{time.time_ns()}"
    coder_token = f"coder-token-{time.time_ns()}"
    verifier_token = f"verifier-token-{time.time_ns()}"
    temp_projects: tempfile.TemporaryDirectory[str] | None = None
    projects_dir = root / "projects"
    if not keep_project:
        temp_projects = tempfile.TemporaryDirectory(prefix="aspis-mcp-smoke-")
        projects_dir = Path(temp_projects.name)
    project_file = projects_dir / f"{project_id}.md"
    write_smoke_project(projects_dir, project_root, project_id)
    seed_launch_pending_agents(
        projects_dir,
        project_id,
        "Oracle Production Readiness",
        {
            architect_id: ("orchestrator", architect_token, None),
            coder_id: ("coder", coder_token, "T1"),
            verifier_id: ("verifier", verifier_token, "T1"),
        },
    )

    required = {
        "agent_register",
        "agent_state",
        "project_list",
        "project_get",
        "project_next_task",
        "project_claim_task",
        "project_update_status",
        "project_append_note",
        "agent_heartbeat",
        "provider_credentials_status",
        "oracle_context",
        "oracle_ask",
        "cloudflare_list_workers",
        "cloudflare_rotate_worker_secret",
        "scaleway_list_resources",
        "scaleway_resource_action",
    }

    env = os.environ.copy()
    env.update(
        {
            "PYTHONIOENCODING": "utf-8",
            "HF_HUB_OFFLINE": "1",
            "TRANSFORMERS_OFFLINE": "1",
            "ORACLE_ASK_DISABLE_LLM": "1",
        }
    )
    params = StdioServerParameters(
        command=sys.executable,
        args=[
            "-m",
            "oracle.server.aspis_mcp",
            "--root",
            str(root),
            "--projects-dir",
            str(projects_dir),
        ],
        cwd=str(root),
        env=env,
    )
    try:
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                tools = await session.list_tools()
                tool_names = {tool.name for tool in tools.tools}
                missing = sorted(required - tool_names)
                if missing:
                    raise AssertionError(f"Missing MCP tools: {missing}")

                architect_registration = await call(
                    session,
                    "agent_register",
                    {
                        "agent_id": architect_id,
                        "role": "architect",
                        "model": "smoke",
                        "message": "starting",
                        "launch_token": architect_token,
                    },
                )
                coder_registration = await call(
                    session,
                    "agent_register",
                    {
                        "agent_id": coder_id,
                        "role": "code",
                        "model": "codex",
                        "message": "coding",
                        "launch_token": coder_token,
                    },
                )
                verifier_registration = await call(
                    session,
                    "agent_register",
                    {
                        "agent_id": verifier_id,
                        "role": "verifier",
                        "model": "verifier",
                        "message": "verifying",
                        "launch_token": verifier_token,
                    },
                )
                architect_session_token = str(
                    architect_registration.get("sessionToken") or ""
                )
                coder_session_token = str(coder_registration.get("sessionToken") or "")
                verifier_session_token = str(
                    verifier_registration.get("sessionToken") or ""
                )
                if (
                    not architect_session_token
                    or not coder_session_token
                    or not verifier_session_token
                ):
                    raise AssertionError(
                        "agent_register did not return sessionToken values."
                    )
                architect = {
                    "agent_id": architect_id,
                    "role": "architect",
                    "session_token": architect_session_token,
                }
                coder = {
                    "agent_id": coder_id,
                    "role": "code",
                    "session_token": coder_session_token,
                }
                verifier = {
                    "agent_id": verifier_id,
                    "role": "verifier",
                    "session_token": verifier_session_token,
                }

                spoof_error = await call(
                    session,
                    "project_list",
                    {
                        "agent_id": architect_id,
                        "role": "architect",
                        "session_token": "wrong-token",
                    },
                    expect_error=True,
                )
                if "session token is invalid" not in spoof_error:
                    raise AssertionError(
                        f"Wrong session token failed for the wrong reason: {spoof_error}"
                    )

                anon_project_error = await call(
                    session,
                    "project_get",
                    {"project_id": project_id},
                    expect_error=True,
                )
                if (
                    "Agent id" not in anon_project_error
                    and "agent_id" not in anon_project_error
                ):
                    raise AssertionError(
                        f"Anonymous project_get failed for the wrong reason: {anon_project_error}"
                    )

                anon_state_error = await call(
                    session, "agent_state", {}, expect_error=True
                )
                if (
                    "Agent id" not in anon_state_error
                    and "agent_id" not in anon_state_error
                ):
                    raise AssertionError(
                        f"Anonymous agent_state failed for the wrong reason: {anon_state_error}"
                    )

                state = await call(
                    session,
                    "agent_state",
                    architect,
                )
                roles = {
                    item.get("agentId"): item.get("role")
                    for item in state.get("sessions", [])
                }
                if (
                    roles.get(architect_id) != "orchestrator"
                    or roles.get(coder_id) != "coder"
                ):
                    raise AssertionError(
                        f"Role aliases did not canonicalize correctly: {roles}"
                    )
                serialized_state = json.dumps(state)
                if (
                    "sessionTokenHash" in serialized_state
                    or "launchTokenHash" in serialized_state
                ):
                    raise AssertionError(f"Agent state leaked token hashes: {state}")

                credential_status = await call(
                    session,
                    "provider_credentials_status",
                    verifier,
                )
                if (
                    "providers" not in credential_status
                    or "oracleLlm" not in credential_status
                ):
                    raise AssertionError(
                        f"Provider credential status shape is invalid: {credential_status}"
                    )
                if has_sensitive_credential_field(credential_status):
                    raise AssertionError(
                        f"Provider credential status leaks secret-shaped fields: {credential_status}"
                    )

                projects = await call(
                    session,
                    "project_list",
                    architect,
                )
                if project_id not in {
                    item.get("id") for item in projects.get("projects", [])
                }:
                    raise AssertionError(
                        "Production project is not visible through project_list."
                    )

                project_get = await call(
                    session,
                    "project_get",
                    {"project_id": project_id, **architect},
                )
                if project_get.get("metadata", {}).get("id") != project_id:
                    raise AssertionError(
                        f"Registered project_get returned the wrong project: {project_get}"
                    )
                if project_get.get("metadata", {}).get("rootPath") != str(project_root):
                    raise AssertionError(
                        f"Registered project_get did not expose the project root: {project_get}"
                    )
                if len(project_get.get("state", {}).get("tasks", [])) != 2:
                    raise AssertionError(
                        f"Registered project_get did not expose tasks: {project_get}"
                    )

                next_task = await call(
                    session,
                    "project_next_task",
                    {"project_id": project_id, **coder},
                )
                if next_task.get("task", {}).get("id") != "T1":
                    raise AssertionError(
                        f"Coder next task did not point to T1: {next_task}"
                    )

                context = await call(
                    session,
                    "oracle_context",
                    {
                        "query": "scaleway rnaseq worker provider lifecycle",
                        "limit": 3,
                        "project_id": project_id,
                        **architect,
                    },
                )
                if not context.get("chunks"):
                    raise AssertionError(
                        "Oracle context returned no chunks for the project root."
                    )

                answer = await call(
                    session,
                    "oracle_ask",
                    {
                        "query": "How do terminal agents update project task status through MCP?",
                        "limit": 5,
                        "project_id": project_id,
                        **architect,
                    },
                    timeout=180,
                )
                if answer.get("not_found") or not answer.get("citations"):
                    raise AssertionError(
                        f"Oracle ask did not return a grounded answer: {answer}"
                    )
                if not answer.get("results"):
                    raise AssertionError(
                        f"Oracle ask did not return result rows: {answer}"
                    )

                await call(
                    session,
                    "project_claim_task",
                    {"project_id": project_id, "task_id": "T1", **coder},
                )
                await call(
                    session,
                    "project_update_status",
                    {
                        "project_id": project_id,
                        "task_id": "T1",
                        "status": "wip",
                        "evidence": "Coder started implementation through real MCP stdio.",
                        "confidence": 0.5,
                        **coder,
                    },
                )
                await call(
                    session,
                    "project_update_status",
                    {
                        "project_id": project_id,
                        "task_id": "T1",
                        "status": "review",
                        "evidence": "Coder finished implementation and handed off review through MCP.",
                        "confidence": 0.72,
                        **coder,
                    },
                )
                await call(
                    session,
                    "project_claim_task",
                    {"project_id": project_id, "task_id": "T1", **verifier},
                )
                done_project = await call(
                    session,
                    "project_update_status",
                    {
                        "project_id": project_id,
                        "task_id": "T1",
                        "status": "done",
                        "evidence": "Verifier confirmed stdio lifecycle, role audit and markdown rewrite.",
                        "confidence": 0.84,
                        **verifier,
                    },
                )
                task = next(
                    item
                    for item in done_project["state"]["tasks"]
                    if item["id"] == "T1"
                )
                if task["status"] != "done":
                    raise AssertionError("Verifier did not close T1.")

                verifier_todo_error = await call(
                    session,
                    "project_claim_task",
                    {"project_id": project_id, "task_id": "T2", **verifier},
                    expect_error=True,
                )
                if (
                    "Verifier agents can only claim review or blocked tasks"
                    not in verifier_todo_error
                ):
                    raise AssertionError(
                        f"Verifier TODO claim failed for the wrong reason: {verifier_todo_error}"
                    )

                coder_cloud_context_error = await call(
                    session,
                    "scaleway_resource_action",
                    {
                        "resource_id": "server-1",
                        "action": "stop",
                        **coder,
                    },
                    expect_error=True,
                )
                if "management_project_id" not in coder_cloud_context_error:
                    raise AssertionError(
                        f"Coder provider mutation was not Kanban-gated: {coder_cloud_context_error}"
                    )

                verifier_cloud_error = await call(
                    session,
                    "scaleway_resource_action",
                    {
                        "resource_id": "server-1",
                        "action": "stop",
                        "management_project_id": project_id,
                        "task_id": "T1",
                        "evidence": "Verifier should be denied cloud mutation.",
                        **verifier,
                    },
                    expect_error=True,
                )
                if (
                    "verifier agents cannot use scaleway_resource_action"
                    not in verifier_cloud_error
                ):
                    raise AssertionError(
                        f"Verifier cloud mutation failed for the wrong reason: {verifier_cloud_error}"
                    )

                architect_cloud_error = await call(
                    session,
                    "cloudflare_rotate_worker_secret",
                    {
                        "worker_name": "worker",
                        "secret_name": "TEST_SECRET",
                        "secret_value": "not-a-real-secret",
                        "management_project_id": project_id,
                        "task_id": "T1",
                        "evidence": "Orchestrator should be denied cloud mutation.",
                        **architect,
                    },
                    expect_error=True,
                )
                if (
                    "orchestrator agents cannot use cloudflare_rotate_worker_secret"
                    not in architect_cloud_error
                ):
                    raise AssertionError(
                        f"Architect cloud mutation failed for the wrong reason: {architect_cloud_error}"
                    )

                provider_reads: dict[str, Any] = {"mode": "skipped", "pass": True}
                if live_providers:
                    provider_reads = {"mode": "live", "pass": True}
                    cloudflare_status = await call_status(
                        session,
                        "cloudflare_list_workers",
                        verifier,
                        timeout=120,
                    )
                    scaleway_status = await call_status(
                        session,
                        "scaleway_list_resources",
                        verifier,
                        timeout=120,
                    )
                    provider_reads["cloudflare"] = summarize_provider_read(
                        cloudflare_status, "workers"
                    )
                    provider_reads["scaleway"] = summarize_provider_read(
                        scaleway_status, "resources"
                    )
                    provider_reads["pass"] = bool(
                        provider_reads["cloudflare"].get("pass")
                        and provider_reads["scaleway"].get("pass")
                    )

                await call(
                    session,
                    "project_claim_task",
                    {"project_id": project_id, "task_id": "T2", **coder},
                )
                review_project = await call(
                    session,
                    "project_update_status",
                    {
                        "project_id": project_id,
                        "task_id": "T2",
                        "status": "review",
                        "evidence": "Oracle retrieval and answer quality are ready for human review.",
                        "confidence": 0.72,
                        **coder,
                    },
                )
                task2 = next(
                    item
                    for item in review_project["state"]["tasks"]
                    if item["id"] == "T2"
                )
                if task2["status"] != "review":
                    raise AssertionError(
                        "Coder did not leave T2 in review for the visible Kanban stage."
                    )

                await call(
                    session,
                    "project_append_note",
                    {
                        "project_id": project_id,
                        "text": "Production smoke verified UI-visible project note flow.",
                        **architect,
                    },
                )
                await call(
                    session,
                    "agent_heartbeat",
                    {
                        "agent_id": architect_id,
                        "status": "coordinating",
                        "message": "watching Kanban state",
                        "session_token": architect_session_token,
                    },
                )
                final_state = await call(
                    session,
                    "agent_state",
                    architect,
                )
                final_sessions = {
                    item.get("agentId"): item
                    for item in final_state.get("sessions", [])
                }
                if (
                    final_sessions.get(coder_id, {}).get("currentProjectId")
                    != project_id
                ):
                    raise AssertionError(
                        f"Final agent session state is not project-linked: {final_sessions}"
                    )
                final_claims = [
                    item
                    for item in final_state.get("claims", [])
                    if item.get("projectId") == project_id
                    and item.get("taskId") == "T2"
                ]
                if not any(item.get("status") == "review" for item in final_claims):
                    raise AssertionError(
                        f"Final claims do not expose T2 review state: {final_claims}"
                    )
                if not any(
                    item.get("eventType") == "note"
                    and item.get("projectId") == project_id
                    for item in final_state.get("events", [])
                ):
                    raise AssertionError(
                        "Final events do not expose project note updates."
                    )
    finally:
        if not keep_project and temp_projects is not None:
            temp_projects.cleanup()

    status = "pass" if provider_reads.get("pass", True) else "fail"
    return {
        "status": status,
        "project_file": str(project_file),
        "project_id": project_id,
        "project_root": str(project_root),
        "kept_project": keep_project,
        "provider_reads": provider_reads,
        "tools_checked": sorted(required),
    }


def summarize_provider_read(
    status: dict[str, Any], collection_key: str
) -> dict[str, Any]:
    if not status.get("pass"):
        return {
            "pass": False,
            "error": status.get("error", "unknown provider read failure"),
        }
    payload = status.get("payload", {})
    collection = payload.get(collection_key, [])
    scope = payload.get("account") or payload.get("project") or {}
    summary = {
        "pass": True,
        "count": len(collection) if isinstance(collection, list) else 0,
        "scope_id": scope.get("id") if isinstance(scope, dict) else None,
        "scope_name": scope.get("name") if isinstance(scope, dict) else None,
    }
    if collection_key == "workers":
        summary["hidden_sibling_count"] = payload.get("hiddenSiblingWorkers")
        if isinstance(collection, list):
            summary["names"] = [
                item.get("name")
                for item in collection
                if isinstance(item, dict) and item.get("name")
            ][:20]
    return summary


def has_sensitive_credential_field(value: Any) -> bool:
    if isinstance(value, dict):
        for key, child in value.items():
            if str(key).lower() in {
                "value",
                "password",
                "secret",
                "secretvalue",
                "api_key",
                "apikey",
            }:
                return True
            if has_sensitive_credential_field(child):
                return True
    if isinstance(value, list):
        return any(has_sensitive_credential_field(item) for item in value)
    return False


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Real MCP stdio smoke for Devboule Projects Kanban agents."
    )
    parser.add_argument("--root", default=".", help="Devboule root")
    parser.add_argument(
        "--project-root",
        default=None,
        help="Root indexed by Oracle for the smoke project",
    )
    parser.add_argument(
        "--keep-project",
        action="store_true",
        help="Leave the production-readiness project visible in the app",
    )
    parser.add_argument(
        "--project-id",
        default="",
        help="Project id to create/use for a kept production smoke project",
    )
    parser.add_argument(
        "--live-providers",
        action="store_true",
        help="Also call Cloudflare/Scaleway read tools with configured credentials",
    )
    args = parser.parse_args()
    root = Path(args.root).resolve()
    default_project_root = Path.home() / "Desktop" / "aspis bio"
    project_root = (
        Path(args.project_root).resolve()
        if args.project_root
        else default_project_root.resolve()
    )
    payload = asyncio.run(
        run(
            root,
            project_root,
            keep_project=args.keep_project,
            live_providers=args.live_providers,
            project_id_override=args.project_id.strip() or None,
        )
    )
    print(json.dumps(payload, indent=2))
    return 0 if payload.get("status") == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
