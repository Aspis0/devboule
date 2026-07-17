#!/usr/bin/env python3
"""
Concurrent-coders rig: two independent agents (main-coder + mini-coder) running
at the same time without corrupting shared state.

Two cells:
  1. test_two_sidecars_run_concurrently
       Two SidecarSession sidecars (one main-coder, one mini-coder) drive their own
       mock LLM servers concurrently. Each mock is scripted to issue a `read` tool_call
       for a distinct marker file, then return a final text. Assert BOTH sessions reach
       response{success:true} within a bounded deadline AND each session's event stream
       contains ONLY its own tool_call/marker (no cross-contamination).

  2. test_concurrent_claims_are_serialized
       Two agents register against the SAME projects_dir and fire `project_claim_task`
       for the SAME task_id concurrently (two threads, each with its own McpStdioClient
       → its own MCP server subprocess). Assert EXACTLY ONE claim wins (the other is
       rejected with McpError "already claimed"). Then a second scenario: project with
       TWO tasks, two agents claim DIFFERENT task_ids concurrently → BOTH succeed.

Gated by RIG=1 (same skip guard as rig/test_task_lifecycle.py).
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Gate: only run when RIG=1
# ---------------------------------------------------------------------------
if os.environ.get("RIG") != "1":
    pytest.skip(
        "RIG=1 required; skipping concurrent-coders rig tests", allow_module_level=True
    )

# ---------------------------------------------------------------------------
# Imports from our own rig modules
# ---------------------------------------------------------------------------
from rig.mcp_client import McpStdioClient, McpError  # noqa: E402
from rig.sidecar_driver import SidecarEvent, SidecarSession  # noqa: E402
from rig.world import make_projects_dir, forge_agent_launch  # noqa: E402
from rig.mock_llm import MockLLMServer  # noqa: E402

# Repo root (two levels up from rig/)
REPO_ROOT = Path(__file__).resolve().parent.parent

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
ACTIVE_PROJECT_ID = "test-proj"
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


def _collect_until_response(
    session: SidecarSession,
    timeout: float = 60.0,
    start_index: int = 0,
) -> tuple:
    """Pump session.events until a NEW response{command:prompt} event arrives.

    Scans from `start_index` onwards — caller MUST pass len(session.events)
    from the previous call to avoid re-matching a stale response event.

    Returns (response_event_or_None, scan_end_index). On timeout, returns
    (None, scan_end_index) so the caller can resume without re-scanning.
    """
    deadline = time.time() + timeout
    last_scanned = start_index
    while time.time() < deadline:
        try:
            event = session.wait_event(
                lambda e: True, timeout=0.5, start_index=last_scanned
            )
        except TimeoutError:
            continue

        last_scanned = session.events.index(event) + 1

        if not session.is_alive():
            raise RuntimeError(
                f"Sidecar process exited unexpectedly (returncode={session._proc.returncode}).\n"
                f"{session.dump_state()}"
            )

        if (
            event.data.get("type") == "response"
            and event.data.get("command") == "prompt"
        ):
            return event, last_scanned
    return None, last_scanned


def _extract_text(event_data: dict) -> str:
    """Extract assistant text content from various pi SDK event shapes."""
    etype = event_data.get("type")

    if etype == "message_update":
        ame = event_data.get("assistantMessageEvent", {})
        if ame:
            return _extract_text(ame)

    if "message" in event_data:
        msg = event_data["message"]
        if (
            isinstance(msg, dict)
            and msg.get("role") == "assistant"
            and "content" in msg
        ):
            content = msg["content"]
            if isinstance(content, list):
                parts = []
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        parts.append(block.get("text", ""))
                return "".join(parts)
            elif isinstance(content, str):
                return content

    if etype == "text_delta":
        return event_data.get("delta", "")

    if etype == "assistant":
        content = event_data.get("content")
        if isinstance(content, list):
            parts = []
            for block in content:
                if isinstance(block, dict) and block.get("type") == "text":
                    parts.append(block.get("text", ""))
            return "".join(parts)

    return ""


# ---------------------------------------------------------------------------
# Cell 1: test_two_sidecars_run_concurrently
# ---------------------------------------------------------------------------


@pytest.mark.rig
def test_two_sidecars_run_concurrently():
    """Two SidecarSession sidecars (main-coder + mini-coder) drive their own
    mock LLM servers concurrently. Each mock is scripted to issue a `read`
    tool_call for a distinct marker file, then return a final text.

    Assert BOTH sessions reach response{success:true} within a bounded deadline
    AND each session's event stream contains ONLY its own tool_call/marker
    (no cross-contamination between the two live sidecars).
    """
    # Distinct markers — each sidecar's mock will ask it to read its own file.
    main_marker = f"MAIN-MARKER-{uuid.uuid4().hex[:12]}"
    mini_marker = f"MINI-MARKER-{uuid.uuid4().hex[:12]}"

    with tempfile.TemporaryDirectory(prefix="rig-concurrent-sidecars-") as tmp_str:
        tmp = Path(tmp_str)

        # --- Build the shared project_root (two marker files planted here) ---
        project_root = tmp / "project"
        project_root.mkdir(parents=True, exist_ok=True)

        # Seed marker files inside the project so the SDK's `read` tool can resolve them.
        main_marker_path = project_root / "main_marker.txt"
        main_marker_path.write_text(
            f"Main coder marker: {main_marker}\n", encoding="utf-8"
        )
        mini_marker_path = project_root / "mini_marker.txt"
        mini_marker_path.write_text(
            f"Mini coder marker: {mini_marker}\n", encoding="utf-8"
        )

        # --- Minimal agent_dir with settings.json ---
        agent_dir = tmp / "agent"
        agent_dir.mkdir(parents=True, exist_ok=True)
        (agent_dir / "settings.json").write_text(
            '{"packages": []}', encoding="utf-8"
        )

        # --- Two independent MockLLMServer instances, one per sidecar ---
        with MockLLMServer() as main_mock:
            with MockLLMServer() as mini_mock:
                # Scripted responses per sidecar:
                #   1) assistant issues a tool_call for `read` on its own marker file
                #   2) assistant produces final text once the tool result returns
                main_scripted_tool = {
                    "tool": "read",
                    "arguments": {"path": str(main_marker_path)},
                }
                main_scripted_final = (
                    f"Read main_marker.txt — the marker is: {main_marker}."
                )
                main_mock.set_responses([main_scripted_tool, main_scripted_final])

                mini_scripted_tool = {
                    "tool": "read",
                    "arguments": {"path": str(mini_marker_path)},
                }
                mini_scripted_final = (
                    f"Read mini_marker.txt — the marker is: {mini_marker}."
                )
                mini_mock.set_responses([mini_scripted_tool, mini_scripted_final])

                # --- Spawn BOTH sidecars concurrently (open both `with` sessions
                #     and send_prompt to both, THEN wait on both) ---
                main_session_id = f"rig-main-{uuid.uuid4().hex[:8]}"
                mini_session_id = f"rig-mini-{uuid.uuid4().hex[:8]}"

                with SidecarSession(
                    session_id=main_session_id,
                    agent_role="main-coder",
                    mock_base_url=main_mock.base_url + "/v1",
                    project_root=project_root,
                    agent_dir=agent_dir,
                    pigeon_enabled=False,
                ) as main_session:
                    with SidecarSession(
                        session_id=mini_session_id,
                        agent_role="mini-coder",
                        mock_base_url=mini_mock.base_url + "/v1",
                        project_root=project_root,
                        agent_dir=agent_dir,
                        pigeon_enabled=False,
                    ) as mini_session:
                        # Wait for both to be ready (bounded deadline).
                        main_ready = main_session.wait_ready(timeout=30.0)
                        assert main_ready.data.get("type") == "ready"
                        mini_ready = mini_session.wait_ready(timeout=30.0)
                        assert mini_ready.data.get("type") == "ready"

                        # Send prompts to BOTH sidecars so they overlap in time.
                        main_session.send_prompt(
                            f"Read {main_marker_path} and confirm the marker."
                        )
                        mini_session.send_prompt(
                            f"Read {mini_marker_path} and confirm the marker."
                        )

                        # Record event counts right after sending prompts — the first
                        # event at or after this index is the first post-prompt event.
                        main_start_idx = len(main_session.events)
                        mini_start_idx = len(mini_session.events)

                        # Round-robin poll BOTH sessions until each reaches a terminal
                        # response, recording wall-clock timestamps for overlap proof.
                        main_first_ts: float | None = None
                        mini_first_ts: float | None = None
                        main_response: SidecarEvent | None = None
                        mini_response: SidecarEvent | None = None
                        main_scan_end = main_start_idx
                        mini_scan_end = mini_start_idx
                        overall_deadline = time.time() + 60.0

                        while time.time() < overall_deadline:
                            # Poll main session.
                            if main_response is None:
                                try:
                                    event = main_session.wait_event(
                                        lambda e: True,
                                        timeout=0.5,
                                        start_index=main_scan_end,
                                    )
                                except TimeoutError:
                                    pass
                                else:
                                    if not main_session.is_alive():
                                        raise RuntimeError(
                                            f"main-coder sidecar exited unexpectedly "
                                            f"(returncode={main_session._proc.returncode}).\n"
                                            f"{main_session.dump_state()}"
                                        )
                                    if main_first_ts is None:
                                        main_first_ts = event.timestamp
                                    main_scan_end = main_session.events.index(event) + 1
                                    if (
                                        event.data.get("type") == "response"
                                        and event.data.get("command") == "prompt"
                                    ):
                                        main_response = event

                            # Poll mini session.
                            if mini_response is None:
                                try:
                                    event = mini_session.wait_event(
                                        lambda e: True,
                                        timeout=0.5,
                                        start_index=mini_scan_end,
                                    )
                                except TimeoutError:
                                    pass
                                else:
                                    if not mini_session.is_alive():
                                        raise RuntimeError(
                                            f"mini-coder sidecar exited unexpectedly "
                                            f"(returncode={mini_session._proc.returncode}).\n"
                                            f"{mini_session.dump_state()}"
                                        )
                                    if mini_first_ts is None:
                                        mini_first_ts = event.timestamp
                                    mini_scan_end = mini_session.events.index(event) + 1
                                    if (
                                        event.data.get("type") == "response"
                                        and event.data.get("command") == "prompt"
                                    ):
                                        mini_response = event

                            # Both done — break out.
                            if main_response is not None and mini_response is not None:
                                break

                        # Assert BOTH sessions reached terminal response with success:true.
                        assert main_response is not None, (
                            f"main-coder sidecar never reached a response.\n"
                            f"{main_session.dump_state()}"
                        )
                        assert main_response.data.get("success") is True, (
                            f"main-coder response failed: {main_response.data.get('error')!r}\n"
                            f"{main_session.dump_state()}"
                        )

                        assert mini_response is not None, (
                            f"mini-coder sidecar never reached a response.\n"
                            f"{mini_session.dump_state()}"
                        )
                        assert mini_response.data.get("success") is True, (
                            f"mini-coder response failed: {mini_response.data.get('error')!r}\n"
                            f"{mini_session.dump_state()}"
                        )

                        # PROVE temporal overlap: both sessions were alive and processing
                        # at the same time. If execution was serialized (one finished
                        # before the other started), this assertion fails.
                        assert main_first_ts is not None, (
                            "main-coder session produced no events after prompt."
                        )
                        assert mini_first_ts is not None, (
                            "mini-coder session produced no events after prompt."
                        )
                        assert (
                            max(main_first_ts, mini_first_ts) < min(
                                main_response.timestamp, mini_response.timestamp
                            )
                        ), (
                            "Sidecars did not overlap in time — execution was serialized, "
                            "not concurrent. "
                            f"main[{main_first_ts:.3f}, {main_response.timestamp:.3f}] "
                            f"mini[{mini_first_ts:.3f}, {mini_response.timestamp:.3f}]"
                        )

                        # --- Session isolation: each session's events contain ONLY
                        #     its own tool_call/marker (no cross-contamination) ---
                        main_events = list(main_session.events)
                        mini_events = list(mini_session.events)

                        # Extract all text from each session's events.
                        main_text = "".join(_extract_text(e.data) for e in main_events)
                        mini_text = "".join(_extract_text(e.data) for e in mini_events)

                        # Main session must contain its own marker and NOT mini's.
                        assert main_marker in main_text, (
                            f"main-coder session missing its own marker {main_marker!r}.\n"
                            f"main_text: {main_text!r}"
                        )
                        assert main_marker not in mini_text, (
                            f"CROSS-CONTAMINATION: mini-coder session contains main marker {main_marker!r}.\n"
                            f"mini_text: {mini_text!r}"
                        )
                        # Mini session must contain its own marker and NOT main's.
                        assert mini_marker in mini_text, (
                            f"mini-coder session missing its own marker {mini_marker!r}.\n"
                            f"mini_text: {mini_text!r}"
                        )
                        assert mini_marker not in main_text, (
                            f"CROSS-CONTAMINATION: main-coder session contains mini marker {mini_marker!r}.\n"
                            f"main_text: {main_text!r}"
                        )

                        # Tool isolation: main session's tool_execution_start events
                        # must reference main_marker_path; mini's must reference mini_marker_path.
                        main_tool_starts = [
                            e
                            for e in main_events
                            if e.data.get("type") == "tool_execution_start"
                            and e.data.get("toolName") == "read"
                        ]
                        mini_tool_starts = [
                            e
                            for e in mini_events
                            if e.data.get("type") == "tool_execution_start"
                            and e.data.get("toolName") == "read"
                        ]
                        assert len(main_tool_starts) == 1, (
                            f"main-coder session has {len(main_tool_starts)} tool_execution_start events for 'read' (expected 1).\n"
                            f"Events: {[e.data.get('type') for e in main_events]}\n"
                            f"{main_session.dump_state()}"
                        )
                        assert len(mini_tool_starts) == 1, (
                            f"mini-coder session has {len(mini_tool_starts)} tool_execution_start events for 'read' (expected 1).\n"
                            f"Events: {[e.data.get('type') for e in mini_events]}\n"
                            f"{mini_session.dump_state()}"
                        )
                        # Assert the args.path matches each session's own marker file.
                        main_path_arg = main_tool_starts[0].data.get("args", {}).get("path")
                        mini_path_arg = mini_tool_starts[0].data.get("args", {}).get("path")
                        assert main_path_arg == str(main_marker_path), (
                            f"main-coder tool args.path mismatch: {main_path_arg!r} != {str(main_marker_path)!r}"
                        )
                        assert mini_path_arg == str(mini_marker_path), (
                            f"mini-coder tool args.path mismatch: {mini_path_arg!r} != {str(mini_marker_path)!r}"
                        )
                        # Cross-check: no tool_execution_start in one session references
                        # the other session's marker path.
                        for e in main_tool_starts:
                            p = e.data.get("args", {}).get("path", "")
                            assert str(mini_marker_path) not in p, (
                                f"CROSS-CONTAMINATION: main-coder tool_execution_start "
                                f"references mini marker path: {p!r}"
                            )
                        for e in mini_tool_starts:
                            p = e.data.get("args", {}).get("path", "")
                            assert str(main_marker_path) not in p, (
                                f"CROSS-CONTAMINATION: mini-coder tool_execution_start "
                                f"references main marker path: {p!r}"
                            )


# ---------------------------------------------------------------------------
# Cell 2: test_concurrent_claims_are_serialized
# ---------------------------------------------------------------------------


@pytest.mark.rig
def test_concurrent_claims_are_serialized():
    """Shared-store integrity: two agents act at the SAME instant on the SAME
    task_id. The claim lease must admit exactly one winner per task.

    Scenario A: project with ONE claimable task (T1), two agents fire
    project_claim_task for T1 concurrently from two threads → assert EXACTLY
    ONE claim succeeds (lease granted) and the OTHER is rejected (already
    leased). Never both.

    Scenario B: project with TWO tasks (T1, T2), two agents claim DIFFERENT
    task_ids concurrently → assert BOTH succeed (concurrency is not
    over-serialized).
    """
    with tempfile.TemporaryDirectory(prefix="rig-concurrent-claims-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        # --- Pre-seed T2 into the project file BEFORE any claims ---
        # make_projects_dir only creates T1; inject T2 while the file is still in
        # its original shape (no notes/status mutations yet) so the pattern match
        # below is reliable.
        project_file = projects_dir / f"{ACTIVE_PROJECT_ID}.md"
        _orig = project_file.read_text(encoding="utf-8")
        if '"id": "T2"' not in _orig:
            t2_entry = (
                ',\n    {\n'
                '      "id": "T2",\n'
                '      "title": "Implement feature Y",\n'
                '      "status": "todo",\n'
                '      "priority": "medium",\n'
                '      "assignee": null,\n'
                '      "due": null,\n'
                '      "linkedResources": [],\n'
                '      "updatedAt": "2026-01-15T00:00:00Z"\n'
                "    }"
            )
            _injected = _orig.replace(
                '"updatedAt": "2026-01-15T00:00:00Z"\n    }\n  ],',
                '"updatedAt": "2026-01-15T00:00:00Z"\n    }' + t2_entry + "\n  ],",
            )
            project_file.write_text(_injected, encoding="utf-8")
            assert '"id": "T2"' in _injected, "T2 injection failed"

        # --- Scenario A: ONE task (T1), two agents, SAME task_id ---
        # Forge launch tokens for two agents.
        agent1_id = "rig-concurrent-agent-1"
        agent1_token = forge_agent_launch(projects_dir, agent1_id, AGENT_ROLE)
        agent2_id = "rig-concurrent-agent-2"
        agent2_token = forge_agent_launch(projects_dir, agent2_id, AGENT_ROLE)

        # Two McpStdioClient instances (each spawns its own MCP server subprocess).
        with McpStdioClient(REPO_ROOT, projects_dir) as client1:
            with McpStdioClient(REPO_ROOT, projects_dir) as client2:
                # Register both agents to get session tokens.
                reg1, _ = client1.call_tool(
                    "agent_register",
                    {
                        "agent_id": agent1_id,
                        "role": AGENT_ROLE,
                        "model": "rig-model",
                        "launch_token": agent1_token,
                    },
                    timeout=15,
                )
                session1 = reg1["sessionToken"]

                reg2, _ = client2.call_tool(
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

                # Shared results container (thread-safe via dict + lock).
                results: dict[str, dict | Exception] = {}
                results_lock = threading.Lock()

                barrier = threading.Barrier(2)

                def _claim_task(
                    client: McpStdioClient,
                    session_token: str,
                    agent_id: str,
                    task_id: str,
                    key: str,
                ) -> None:
                    barrier.wait(timeout=15.0)
                    try:
                        result = client.call_tool(
                            "project_claim_task",
                            {
                                "project_id": ACTIVE_PROJECT_ID,
                                "task_id": task_id,
                                "agent_id": agent_id,
                                "role": AGENT_ROLE,
                                "session_token": session_token,
                            },
                            timeout=30,
                        )
                        # call_tool returns (result_dict, raw_len)
                        result_dict = result[0] if isinstance(result, tuple) else result
                        with results_lock:
                            results[key] = result_dict
                    except McpError as exc:
                        with results_lock:
                            results[key] = exc
                    except Exception as exc:
                        # Non-McpError exceptions (e.g. subprocess failure) are
                        # recorded so the failure is visible, not silently lost.
                        with results_lock:
                            results[key] = exc

                # Fire BOTH claims for T1 concurrently from two threads.
                deadline = time.time() + 30.0
                t1 = threading.Thread(
                    target=_claim_task,
                    args=(client1, session1, agent1_id, "T1", "agent1"),
                )
                t2 = threading.Thread(
                    target=_claim_task,
                    args=(client2, session2, agent2_id, "T1", "agent2"),
                )
                t1.start()
                t2.start()

                # Join with bounded timeout.
                t1.join(timeout=25.0)
                t2.join(timeout=25.0)

                # If any thread is still alive after timeout, it's a hang — fail.
                assert not t1.is_alive(), (
                    "agent1 claim thread did not complete within deadline"
                )
                assert not t2.is_alive(), (
                    "agent2 claim thread did not complete within deadline"
                )

                # Assert: EXACTLY ONE claim succeeds, the OTHER is rejected.
                assert len(results) == 2, (
                    f"Expected 2 results (one per thread), got {len(results)}: {list(results.keys())}"
                )
                agent1_result = results.get("agent1")
                agent2_result = results.get("agent2")

                # Determine which one succeeded and which was rejected.
                successes = []
                rejections = []
                for key, val in results.items():
                    if isinstance(val, McpError):
                        rejections.append((key, val))
                    else:
                        successes.append((key, val))

                assert len(successes) == 1, (
                    f"Expected EXACTLY ONE claim success, got {len(successes)}: "
                    f"{[(k, v.get('claim', {}).get('taskId')) for k, v in successes]}. "
                    f"Rejections: {[(k, str(e)) for k, e in rejections]}"
                )
                assert len(rejections) == 1, (
                    f"Expected EXACTLY ONE claim rejection, got {len(rejections)}: "
                    f"{[(k, str(e)) for k, e in rejections]}"
                )

                # The rejection must be "already claimed" (lease conflict).
                rejected_key, rejected_exc = rejections[0]
                assert "already claimed" in str(rejected_exc).lower(), (
                    f"Expected 'already claimed' rejection, got: {rejected_exc}"
                )

                # The winner's claim must carry the expected fields.
                winner_key, winner_result = successes[0]
                claim = winner_result.get("claim", {})
                assert claim.get("taskId") == "T1"
                assert claim.get("projectId") == ACTIVE_PROJECT_ID
                assert claim.get("leaseUntil")

                # On-disk truth: exactly 1 claim for T1.
                disk_state = _read_agents_state(projects_dir)
                t1_claims = [
                    c
                    for c in disk_state.get("claims", [])
                    if c.get("taskId") == "T1"
                    and c.get("projectId") == ACTIVE_PROJECT_ID
                ]
                assert len(t1_claims) == 1, (
                    f"Expected exactly 1 claim for T1 on disk, got {len(t1_claims)}"
                )

        # --- Scenario B: TWO tasks, two agents, DIFFERENT task_ids ---
        # Both T1 and T2 are seeded above; reset the claims list so scenario A's
        # winner claim doesn't block scenario B's agents.
        agents_state_path = projects_dir / ".aspis-agents.json"
        if agents_state_path.exists():
            state = json.loads(agents_state_path.read_text(encoding="utf-8"))
            state["claims"] = []
            agents_state_path.write_text(json.dumps(state, indent=2), encoding="utf-8")

        # Forge tokens for two more agents (fresh identities).
        agent3_id = "rig-concurrent-agent-3"
        agent3_token = forge_agent_launch(projects_dir, agent3_id, AGENT_ROLE)
        agent4_id = "rig-concurrent-agent-4"
        agent4_token = forge_agent_launch(projects_dir, agent4_id, AGENT_ROLE)

        with McpStdioClient(REPO_ROOT, projects_dir) as client3:
            with McpStdioClient(REPO_ROOT, projects_dir) as client4:
                # Register both agents.
                reg3, _ = client3.call_tool(
                    "agent_register",
                    {
                        "agent_id": agent3_id,
                        "role": AGENT_ROLE,
                        "model": "rig-model",
                        "launch_token": agent3_token,
                    },
                    timeout=15,
                )
                session3 = reg3["sessionToken"]

                reg4, _ = client4.call_tool(
                    "agent_register",
                    {
                        "agent_id": agent4_id,
                        "role": AGENT_ROLE,
                        "model": "rig-model",
                        "launch_token": agent4_token,
                    },
                    timeout=15,
                )
                session4 = reg4["sessionToken"]

                concurrent_results: dict[str, dict | Exception] = {}
                concurrent_lock = threading.Lock()

                barrier_b = threading.Barrier(2)

                def _claim_task_b(
                    client: McpStdioClient,
                    session_token: str,
                    agent_id: str,
                    task_id: str,
                    key: str,
                ) -> None:
                    barrier_b.wait(timeout=15.0)
                    try:
                        result = client.call_tool(
                            "project_claim_task",
                            {
                                "project_id": ACTIVE_PROJECT_ID,
                                "task_id": task_id,
                                "agent_id": agent_id,
                                "role": AGENT_ROLE,
                                "session_token": session_token,
                            },
                            timeout=30,
                        )
                        # call_tool returns (result_dict, raw_len)
                        result_dict = result[0] if isinstance(result, tuple) else result
                        with concurrent_lock:
                            concurrent_results[key] = result_dict
                    except McpError as exc:
                        with concurrent_lock:
                            concurrent_results[key] = exc
                    except Exception as exc:
                        # Non-McpError exceptions (e.g. subprocess failure) are
                        # recorded so the failure is visible, not silently lost.
                        with concurrent_lock:
                            concurrent_results[key] = exc

                # Agent3 claims T2, agent4 claims T1 — concurrently (different task_ids).
                t3 = threading.Thread(
                    target=_claim_task_b,
                    args=(client3, session3, agent3_id, "T2", "agent3"),
                )
                t4 = threading.Thread(
                    target=_claim_task_b,
                    args=(client4, session4, agent4_id, "T1", "agent4"),
                )
                t3.start()
                t4.start()

                t3.join(timeout=25.0)
                t4.join(timeout=25.0)

                assert not t3.is_alive(), (
                    "agent3 claim thread did not complete within deadline"
                )
                assert not t4.is_alive(), (
                    "agent4 claim thread did not complete within deadline"
                )

                # Assert: BOTH claims succeed (different task_ids → no conflict).
                assert len(concurrent_results) == 2, (
                    f"Expected 2 results, got {len(concurrent_results)}"
                )
                successes_b = [
                    (k, v) for k, v in concurrent_results.items() if not isinstance(v, McpError)
                ]
                rejections_b = [
                    (k, v) for k, v in concurrent_results.items() if isinstance(v, McpError)
                ]
                assert len(successes_b) == 2, (
                    f"Expected BOTH claims to succeed (different task_ids), got {len(successes_b)} successes, {len(rejections_b)} rejections: {rejections_b}"
                )
                assert len(rejections_b) == 0, (
                    f"Expected NO rejections for different task_ids, got: {rejections_b}"
                )

                # Verify each claim is for the correct task_id and carries a lease.
                for key, result in successes_b:
                    claim = result.get("claim", {})
                    if key == "agent3":
                        assert claim.get("taskId") == "T2", (
                            f"agent3 should claim T2, got {claim.get('taskId')}"
                        )
                    elif key == "agent4":
                        assert claim.get("taskId") == "T1", (
                            f"agent4 should claim T1, got {claim.get('taskId')}"
                        )
                    assert claim.get("leaseUntil"), (
                        f"Claim for {key} succeeded but has no leaseUntil. "
                        f"claim={claim!r}"
                    )

                # On-disk truth: 2 distinct claims for T1 and T2.
                disk_state_b = _read_agents_state(projects_dir)
                t1_claims_b = [
                    c
                    for c in disk_state_b.get("claims", [])
                    if c.get("taskId") == "T1"
                    and c.get("projectId") == ACTIVE_PROJECT_ID
                ]
                t2_claims_b = [
                    c
                    for c in disk_state_b.get("claims", [])
                    if c.get("taskId") == "T2"
                    and c.get("projectId") == ACTIVE_PROJECT_ID
                ]
                assert len(t1_claims_b) == 1, (
                    f"Expected exactly 1 claim for T1 on disk, got {len(t1_claims_b)}"
                )
                assert len(t2_claims_b) == 1, (
                    f"Expected exactly 1 claim for T2 on disk, got {len(t2_claims_b)}"
                )
