from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Any

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


def text_payload(result: Any) -> str:
    return "\n".join(getattr(item, "text", "") for item in result.content).strip()


def json_payload(result: Any) -> dict[str, Any]:
    text = text_payload(result)
    return json.loads(text) if text else {}


async def call(session: ClientSession, name: str, args: dict[str, Any], timeout: int = 90) -> dict[str, Any]:
    result = await asyncio.wait_for(session.call_tool(name, args), timeout=timeout)
    if bool(getattr(result, "isError", False)):
        raise RuntimeError(f"{name} failed: {text_payload(result)}")
    return json_payload(result)


async def run(args: argparse.Namespace) -> dict[str, Any]:
    root = Path(args.root).resolve()
    projects_dir = Path(args.projects_dir).resolve() if args.projects_dir else root / "projects"
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
        args=["-m", "oracle.server.aspis_mcp", "--root", str(root), "--projects-dir", str(projects_dir)],
        cwd=str(root),
        env=env,
    )
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            registration = await call(
                session,
                "agent_register",
                {
                    "agent_id": args.agent_id,
                    "role": args.role,
                    "model": args.model,
                    "message": f"{args.action} probe starting",
                    "launch_token": args.launch_token,
                },
            )
            session_token = str(registration.get("sessionToken") or "")
            if not session_token:
                raise RuntimeError("agent_register did not return sessionToken")
            agent = {"agent_id": args.agent_id, "role": args.role, "session_token": session_token}
            project = await call(session, "project_get", {"project_id": args.project_id, **agent})
            context = await call(
                session,
                "oracle_context",
                {
                    "query": args.query,
                    "project_id": args.project_id,
                    "limit": 3,
                    **agent,
                },
            )
            if not context.get("chunks"):
                raise RuntimeError("oracle_context returned no chunks")

            if args.action == "orchestrate":
                await call(session, "project_claim_task", {"project_id": args.project_id, "task_id": args.task_id, **agent})
                updated = await call(
                    session,
                    "project_update_status",
                    {
                        "project_id": args.project_id,
                        "task_id": args.task_id,
                        "status": "blocked",
                        "evidence": "Real subagent orchestrator used MCP to read Oracle and mark this coordination task blocked for follow-up.",
                        "confidence": 0.74,
                        **agent,
                    },
                )
                await call(
                    session,
                    "project_append_note",
                    {
                        "project_id": args.project_id,
                        "text": "Real subagent orchestrator completed MCP project read, Oracle context read, claim, status update, and note append.",
                        **agent,
                    },
                )
            elif args.action == "code":
                await call(session, "project_claim_task", {"project_id": args.project_id, "task_id": args.task_id, **agent})
                await call(
                    session,
                    "project_update_status",
                    {
                        "project_id": args.project_id,
                        "task_id": args.task_id,
                        "status": "wip",
                        "evidence": "Real subagent coder claimed the task through MCP.",
                        "confidence": 0.55,
                        **agent,
                    },
                )
                updated = await call(
                    session,
                    "project_update_status",
                    {
                        "project_id": args.project_id,
                        "task_id": args.task_id,
                        "status": "review",
                        "evidence": "Real subagent coder used MCP and left the task ready for verifier review.",
                        "confidence": 0.76,
                        **agent,
                    },
                )
            elif args.action == "verify":
                await call(session, "project_claim_task", {"project_id": args.project_id, "task_id": args.task_id, **agent})
                updated = await call(
                    session,
                    "project_update_status",
                    {
                        "project_id": args.project_id,
                        "task_id": args.task_id,
                        "status": "done",
                        "evidence": "Real subagent verifier used MCP to validate the review task and close it.",
                        "confidence": 0.86,
                        **agent,
                    },
                )
            else:
                raise RuntimeError(f"Unsupported action: {args.action}")

            tasks = {
                task.get("id"): task.get("status")
                for task in updated.get("state", {}).get("tasks", [])
            }
            return {
                "status": "pass",
                "agent_id": args.agent_id,
                "role": registration.get("role") or args.role,
                "project_id": project.get("metadata", {}).get("id"),
                "action": args.action,
                "task_statuses": tasks,
                "index_status": context.get("indexStatus") or context.get("index_status"),
                "context_files": [chunk.get("file_source") or chunk.get("fileSource") for chunk in context.get("chunks", [])],
            }


def main() -> int:
    parser = argparse.ArgumentParser(description="Single real-agent MCP role probe.")
    parser.add_argument("--root", default=".")
    parser.add_argument("--projects-dir", default="")
    parser.add_argument("--project-id", required=True)
    parser.add_argument("--task-id", required=True)
    parser.add_argument("--agent-id", required=True)
    parser.add_argument("--role", required=True)
    parser.add_argument("--launch-token", required=True)
    parser.add_argument("--action", choices=["orchestrate", "code", "verify"], required=True)
    parser.add_argument("--model", default="subagent-probe")
    parser.add_argument("--query", default="How do agents update project status through MCP?")
    args = parser.parse_args()
    payload = asyncio.run(run(args))
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
