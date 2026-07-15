#!/usr/bin/env python3
"""
Kanban task-lifecycle rig for the headless MCP.

Drives oracle/server/aspis_mcp.py over stdio, exercising the kanban surface:
  1. Happy path: register → next_task → claim → wip → (verifier) review → done
     → project_get confirms "done" + followup creates a new todo.
  2. dependsOn round-trip: create plan tasks with DAG edges, assert remapped
     dependsOn by TITLE→id lookup; a cyclic DAG is rejected with McpError.
  3. Double-claim rejected: a second agent claims the same task → McpError.
  4. update_status requires valid session: bogus session_token → rejected.

Gated by RIG=1 (same as test_smoke / test_mcp_choreography).
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Gate: only run when RIG=1
# ---------------------------------------------------------------------------
if os.environ.get("RIG") != "1":
    pytest.skip(
        "RIG=1 required; skipping MCP task-lifecycle tests", allow_module_level=True
    )

# ---------------------------------------------------------------------------
# Imports from our own rig modules
# ---------------------------------------------------------------------------
from rig.mcp_client import McpStdioClient, McpError  # noqa: E402
from rig.world import make_projects_dir, forge_agent_launch  # noqa: E402

# Repo root (two levels up from rig/)
REPO_ROOT = Path(__file__).resolve().parent.parent

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
ACTIVE_PROJECT_ID = "test-proj"
AGENT_ID = "rig-test-agent"
AGENT_ROLE = "coder"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _read_agents_state(projects_dir: Path) -> dict:
    """Read the .aspis-agents.json file directly for assertions."""
    path = projects_dir / ".aspis-agents.json"
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return {"sessions": [], "claims": [], "events": []}


def _seed_plan_approval(projects_dir: Path, plan_id: str, status: str) -> None:
    """Append a plan-approval request with the given terminal status to the
    agents state, so the B4/W6 gate can resolve a real verdict.

    Mirrors the queue-entry shape that plan_submit writes + the human
    approve/reject command stamps. The plan_id must be exactly 32 lowercase
    hex characters to pass the plan_id regex gate.
    """
    from oracle.server.aspis_mcp import file_lock, read_agents_state, write_agents_state

    state_lock = projects_dir / ".aspis-agents.json.lock"
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        state.setdefault("planApprovalRequests", []).append(
            {
                "id": plan_id,
                "agentId": AGENT_ID,
                "projectId": ACTIVE_PROJECT_ID,
                "title": "Rig test plan",
                "status": status,
                "createdAt": "2026-01-01T00:00:00Z",
            }
        )
        write_agents_state(projects_dir, state)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.rig
def test_task_lifecycle_to_done():
    """The kanban happy path: register → next_task → claim → wip → verifier
    review → done → project_get confirms "done" + followup creates a new todo.

    `done` requires a verifier role (validate_transition enforces this), so we
    register a coder (for claim/wip) and a verifier (for review/done).
    """
    with tempfile.TemporaryDirectory(prefix="rig-lifecycle-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)
        root = tmp

        # Forge launch tokens for two agents: a coder and a verifier.
        coder_token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)
        verifier_id = "rig-test-verifier"
        verifier_token = forge_agent_launch(projects_dir, verifier_id, "verifier")

        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            # ---- Step 1: register coder ----
            reg, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": coder_token,
                },
                timeout=15,
            )
            coder_session = reg["sessionToken"]

            # ---- Step 2: register verifier ----
            ver_reg, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": verifier_id,
                    "role": "verifier",
                    "model": "rig-model",
                    "launch_token": verifier_token,
                },
                timeout=15,
            )
            verifier_session = ver_reg["sessionToken"]

            # ---- Step 3: project_next_task suggests an open task ----
            next_result, _ = client.call_tool(
                "project_next_task",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": coder_session,
                },
                timeout=10,
            )
            suggested = next_result.get("task")
            assert suggested is not None, (
                f"expected next_task to suggest a task; got {next_result}"
            )
            task_id = suggested["id"]
            assert task_id, "suggested task must have an id"

            # ---- Step 4: coder claims todo→wip ----
            # Status machine: coder must wip→review; only a verifier claim on a
            # review task can close it (done). So: (a) coder claims todo→wip,
            # (b) coder sets review, (c) verifier claims review → done.
            claim_result, _ = client.call_tool(
                "project_claim_task",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "task_id": task_id,
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": coder_session,
                },
                timeout=10,
            )
            claim = claim_result.get("claim")
            assert claim is not None, f"claim ack missing 'claim' key; {claim_result}"
            assert claim.get("projectId") == ACTIVE_PROJECT_ID
            assert claim.get("taskId") == task_id
            assert claim.get("status") in ("claimed", "wip"), (
                f"unexpected claim status: {claim.get('status')}"
            )
            assert claim.get("leaseUntil"), "claim ack must carry leaseUntil"

            # ---- Step 4b: coder sets review ----
            client.call_tool(
                "project_update_status",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "task_id": task_id,
                    "status": "review",
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "evidence": "review evidence text here",
                    "confidence": 0.9,
                    "session_token": coder_session,
                },
                timeout=10,
            )

            # ---- Step 5: verifier claims review (allowed) ----
            ver_claim, _ = client.call_tool(
                "project_claim_task",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "task_id": task_id,
                    "agent_id": verifier_id,
                    "role": "verifier",
                    "session_token": verifier_session,
                },
                timeout=10,
            )
            assert ver_claim.get("claim", {}).get("taskId") == task_id

            # ---- Step 6: project_create_followup (project still active at this point) ----
            followup_result, _ = client.call_tool(
                "project_create_followup",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "title": "Follow-up task",
                    "reason": "Need to revisit after done",
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": coder_session,
                },
                timeout=10,
            )
            assert followup_result, "followup should return something"

            # ---- Step 7: verifier sets done (needs confidence >= 0.70) ----
            client.call_tool(
                "project_update_status",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "task_id": task_id,
                    "status": "done",
                    "agent_id": verifier_id,
                    "role": "verifier",
                    "evidence": "verifier done evidence text",
                    "confidence": 0.95,
                    "session_token": verifier_session,
                },
                timeout=10,
            )

            # ---- Step 8: project_get confirms "done" on the board + followup present ----
            proj_result, _ = client.call_tool(
                "project_get",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": coder_session,
                },
                timeout=10,
            )
            proj = proj_result
            # public_project exposes the project dict directly (metadata + state).
            tasks = proj.get("state", {}).get("tasks", [])
            task_map = {t["id"]: t for t in tasks}
            assert task_id in task_map, (
                f"task {task_id} not found in project; tasks: {[t['id'] for t in tasks]}"
            )
            assert task_map[task_id]["status"] == "done", (
                f"expected task {task_id} status 'done', got {task_map[task_id]['status']}"
            )
            # The followup todo task is still present (project went back to active
            # since not all tasks are done — that's expected).
            task_ids = [t["id"] for t in tasks]
            followup_task = next(
                (t for t in tasks if t.get("title") == "Follow-up task"), None
            )
            assert followup_task is not None, (
                f"followup task 'Follow-up task' not found; tasks: {task_ids}"
            )
            assert followup_task["status"] == "todo", (
                f"followup task must be 'todo', got {followup_task['status']}"
            )


@pytest.mark.rig
def test_task_deps_roundtrip_for_arrows():
    """The arrows surface: create plan tasks WITH dependsOn via
    project_create_plan_tasks (after plan_submit + approval), assert the
    returned tasks carry the expected dependsOn edges remapped through
    T<n> ids. Assert by TITLE→id lookup, not hardcoded plan-internal ids.
    A cyclic DAG is rejected with McpError and state is unchanged.

    project_create_plan_tasks requires an APPROVED plan (B4/W6 gate at
    aspis_mcp.py:8700-8710) — we seed the approval explicitly via _seed_plan_approval.
    """
    with tempfile.TemporaryDirectory(prefix="rig-deps-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)
        root = tmp

        # Forge a launch token for the orchestrator (the role that owns plan_submit).
        orch_token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)

        # A valid 32-lowercase-hex plan id (uuid4().hex shape).
        plan_id = "a" * 32

        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            # ---- Step 1: register the orchestrator ----
            reg, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": orch_token,
                },
                timeout=15,
            )
            session = reg["sessionToken"]

            # ---- Step 2: submit a plan (plan_submit blocks until approved;
            # we pre-seed the approval so it returns immediately). ----
            # project_create_plan_tasks requires an approved plan; seed it first.
            _seed_plan_approval(projects_dir, plan_id, "approved")

            # ---- Step 3: create plan tasks with dependsOn edges ----
            # Plan uses INTERNAL ids "a"/"b"/"c" with b→a, c→b (a chain).
            # The project already has a manual T1, so allocation starts at T2.
            result, _ = client.call_tool(
                "project_create_plan_tasks",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "plan_id": plan_id,
                    "tasks": [
                        {
                            "id": "a",
                            "title": "Scaffold module",
                            "scope": ["src/a.ts"],
                            "acceptance": "builds",
                            "dependsOn": [],
                        },
                        {
                            "id": "b",
                            "title": "Wire it up",
                            "scope": ["src/b.ts"],
                            "acceptance": "tests pass",
                            "dependsOn": ["a"],
                        },
                        {
                            "id": "c",
                            "title": "Add docs",
                            "scope": ["docs/api.md"],
                            "acceptance": "docs built",
                            "dependsOn": ["b"],
                        },
                    ],
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session,
                },
                timeout=15,
            )
            resp = result
            assert resp.get("planId") == plan_id
            # Fresh, non-colliding ids: T1 is the manual task, plan gets T2/T3/T4.
            assert resp["idMap"] == {"a": "T2", "b": "T3", "c": "T4"}, (
                f"unexpected idMap: {resp.get('idMap')}"
            )
            created = resp["tasks"]
            assert [t["id"] for t in created] == ["T2", "T3", "T4"]

            # ---- Step 4: project_get → assert dependsOn by TITLE→id lookup ----
            proj_result, _ = client.call_tool(
                "project_get",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session,
                },
                timeout=10,
            )
            proj = proj_result
            tasks = proj.get("state", {}).get("tasks", [])
            # Build TITLE→task map for lookup by title (not hardcoded ids).
            title_map = {t["title"]: t for t in tasks}
            assert "Scaffold module" in title_map
            assert "Wire it up" in title_map
            assert "Add docs" in title_map
            scaffold = title_map["Scaffold module"]
            wire = title_map["Wire it up"]
            docs = title_map["Add docs"]
            # Root task: EMPTY dependsOn is OMITTED (no-churn).
            assert scaffold.get("dependsOn", []) == []
            assert "dependsOn" not in scaffold
            # Chain: wire depends on scaffold, docs depends on wire.
            assert wire["dependsOn"] == [scaffold["id"]]
            assert docs["dependsOn"] == [wire["id"]]
            # All plan tasks are todo + carry planId.
            for t in [scaffold, wire, docs]:
                assert t["status"] == "todo", f"{t['title']} must be todo"
                assert t.get("planId") == plan_id, f"{t['title']} must carry planId"

            # ---- Step 5: CYCLE is rejected with McpError, state unchanged ----
            # Seed a fresh plan id for the cyclic plan.
            cycle_plan_id = "b" * 32
            _seed_plan_approval(projects_dir, cycle_plan_id, "approved")
            with pytest.raises(McpError, match="cycle"):
                client.call_tool(
                    "project_create_plan_tasks",
                    {
                        "project_id": ACTIVE_PROJECT_ID,
                        "plan_id": cycle_plan_id,
                        "tasks": [
                            {
                                "id": "x",
                                "title": "Task X",
                                "scope": ["src/x.ts"],
                                "acceptance": "x",
                                "dependsOn": ["y"],
                            },
                            {
                                "id": "y",
                                "title": "Task Y",
                                "scope": ["src/y.ts"],
                                "acceptance": "y",
                                "dependsOn": ["x"],
                            },
                        ],
                        "agent_id": AGENT_ID,
                        "role": AGENT_ROLE,
                        "session_token": session,
                    },
                    timeout=10,
                )
            # State unchanged: the same three plan tasks still exist.
            proj_result2, _ = client.call_tool(
                "project_get",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session,
                },
                timeout=10,
            )
            tasks2 = proj_result2.get("state", {}).get("tasks", [])
            titles2 = {t["title"] for t in tasks2}
            assert "Scaffold module" in titles2
            assert "Wire it up" in titles2
            assert "Add docs" in titles2


@pytest.mark.rig
def test_double_claim_rejected():
    """Second register (different agent id, own forged token) claims the SAME
    task already claimed by agent 1 → rejected with McpError.

    The claim handler (aspis_mcp.py:8245-8250) checks active_claim_for_task and
    raises at 8249:
        f"Task is already claimed by {existing_claim.get('agentId')} until
        {existing_claim.get('leaseUntil')}."
    This is the CURRENT contract: a conflicting active claim is REJECTED
    (no takeover). Pin line numbers in the comment above.
    """
    with tempfile.TemporaryDirectory(prefix="rig-double-claim-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)
        root = tmp

        # Forge tokens for two agents.
        agent1_token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)
        agent2_id = "rig-test-agent-2"
        agent2_token = forge_agent_launch(projects_dir, agent2_id, AGENT_ROLE)

        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            # ---- Step 1: register both agents ----
            reg1, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": agent1_token,
                },
                timeout=15,
            )
            session1 = reg1["sessionToken"]

            reg2, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": agent2_id,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": agent2_token,
                },
                timeout=15,
            )
            session2 = reg2["sessionToken"]

            # ---- Step 2: agent 1 claims T1 ----
            claim1, _ = client.call_tool(
                "project_claim_task",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "task_id": "T1",
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session1,
                },
                timeout=10,
            )
            assert claim1.get("claim", {}).get("taskId") == "T1"

            # ---- Step 3: agent 2 tries to claim T1 → rejected ----
            with pytest.raises(McpError, match="already claimed"):
                client.call_tool(
                    "project_claim_task",
                    {
                        "project_id": ACTIVE_PROJECT_ID,
                        "task_id": "T1",
                        "agent_id": agent2_id,
                        "role": AGENT_ROLE,
                        "session_token": session2,
                    },
                    timeout=10,
                )

            # ---- Step 4: on-disk truth: exactly 1 claim for T1 ----
            disk_state = _read_agents_state(projects_dir)
            t1_claims = [
                c
                for c in disk_state.get("claims", [])
                if c.get("taskId") == "T1" and c.get("projectId") == ACTIVE_PROJECT_ID
            ]
            assert len(t1_claims) == 1, (
                f"expected exactly 1 claim for T1 on disk, got {len(t1_claims)}"
            )
            assert t1_claims[0].get("agentId") == AGENT_ID


@pytest.mark.rig
def test_update_status_requires_valid_session():
    """project_update_status with a bogus session_token → rejected with McpError;
    task status unchanged on disk. We claim the task first (with the valid
    session) so the bogus-session rejection is the only reason for failure."""
    with tempfile.TemporaryDirectory(prefix="rig-bogus-session-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)
        root = tmp

        token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)

        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            # ---- Step 1: register ----
            reg, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": token,
                },
                timeout=15,
            )
            valid_session = reg["sessionToken"]

            # ---- Step 2: claim T1 (so the task is locked to our session) ----
            client.call_tool(
                "project_claim_task",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "task_id": "T1",
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": valid_session,
                },
                timeout=10,
            )

            # ---- Step 3: bogus session_token → rejected (session validation fires) ----
            with pytest.raises(McpError):
                client.call_tool(
                    "project_update_status",
                    {
                        "project_id": ACTIVE_PROJECT_ID,
                        "task_id": "T1",
                        "status": "review",
                        "agent_id": AGENT_ID,
                        "role": AGENT_ROLE,
                        "evidence": "bogus evidence text",
                        "confidence": 0.9,
                        "session_token": "totally-bogus-session",
                    },
                    timeout=10,
                )

            # ---- Step 4: task status unchanged on disk (still "wip" from the claim) ----
            proj_path = projects_dir / f"{ACTIVE_PROJECT_ID}.md"
            assert proj_path.exists(), f"project file missing: {proj_path}"
            content = proj_path.read_text(encoding="utf-8")
            # The claim auto-advanced T1 to "wip"; the bogus session must NOT have
            # advanced it further to "review".
            assert '"status": "wip"' in content, (
                f"T1 status was mutated to 'review' despite bogus session; content:\n{content}"
            )

            # ---- Step 5: valid session still works (claim + review) ----
            client.call_tool(
                "project_update_status",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "task_id": "T1",
                    "status": "review",
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "evidence": "valid evidence text here",
                    "confidence": 0.9,
                    "session_token": valid_session,
                },
                timeout=10,
            )
            # Now T1 should be "review" on disk.
            content2 = proj_path.read_text(encoding="utf-8")
            assert '"status": "review"' in content2, (
                f"T1 status should be 'review' after valid session; content:\n{content2}"
            )
