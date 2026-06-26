"""Slice 3 ON-PATH integration: the aspis_mcp Pigeon transport helpers driven against a
REAL, booted Pigeon dispatcher (a `python -m pigeon.dispatcher` subprocess on an ephemeral
loopback port). Proves the protocol matches the dispatcher end-to-end:

  * `_pigeon_send_directive` POSTs the WHOLE directive to `receiver_id="mini-pool"` and
    returns the assigned `ticket_no`;
  * a simulated worker claims it via `/poll` (the same call the Rust executor makes) and
    completes it via `/done`;
  * `_await_mini_directive_pigeon` then returns `{directiveId, result}` with the executor's
    posted outcome — the SAME shape the file path returns.

This complements the BYTE-IDENTICAL flag-OFF unit test in `test_aspis_mcp.py`
(`PigeonFlagOffByteIdenticalTests`).

Run: oracle-data/venv/bin/python -m pytest oracle/tests/test_aspis_mcp_pigeon.py
"""
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

import httpx
import pytest

import oracle.server.aspis_mcp as mcp

PROJECT_ROOT = Path(__file__).resolve().parents[2]
AUTH = "test-pigeon-token"


def _free_port() -> int:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


@pytest.fixture
def dispatcher(tmp_path, monkeypatch):
    """Boot a real Pigeon dispatcher subprocess; yield (base_url, headers). Sets the
    PIGEON_* env so `_pigeon_enabled()` is True and the helpers target this instance."""
    port = _free_port()
    sqlite_path = tmp_path / "mailbox.sqlite"
    env = dict(os.environ)
    env["PIGEON_PORT"] = str(port)
    env["PIGEON_AUTH_TOKEN"] = AUTH
    env["PIGEON_SQLITE_PATH"] = str(sqlite_path)
    env["PIGEON_DIR"] = str(tmp_path)
    env["PYTHONPATH"] = str(PROJECT_ROOT)

    proc = subprocess.Popen(
        [sys.executable, "-m", "pigeon.dispatcher"],
        cwd=str(PROJECT_ROOT),
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    base = f"http://127.0.0.1:{port}"
    headers = {"x-pigeon-auth-token": AUTH}
    # Wait for readiness (bounded). If the child died, surface its output.
    deadline = time.monotonic() + 15.0
    ready = False
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            out = proc.stdout.read().decode("utf-8", "replace") if proc.stdout else ""
            raise RuntimeError(f"pigeon dispatcher exited early ({proc.returncode}):\n{out}")
        try:
            r = httpx.get(f"{base}/health", timeout=1.0)
            if r.status_code == 200:
                ready = True
                break
        except Exception:
            time.sleep(0.1)
    if not ready:
        proc.terminate()
        raise RuntimeError("pigeon dispatcher did not become ready in time")

    # Point the aspis_mcp helpers at this dispatcher.
    monkeypatch.setenv("PIGEON_PORT", str(port))
    monkeypatch.setenv("PIGEON_AUTH_TOKEN", AUTH)
    try:
        yield base, headers
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


def test_pigeon_enabled_true_with_booted_dispatcher(dispatcher):
    assert mcp._pigeon_enabled() is True


def test_send_directive_targets_mini_pool_and_round_trips(dispatcher):
    base, headers = dispatcher
    directive = {
        "id": "dir-abc",
        "parentAgentId": "coder-1",
        "status": "pending",
        "task": "tidy a docstring",
        "files": ["src/a.py"],
        "resultPath": "dir-abc.json",
        "createdAt": "2026-06-26T00:00:00Z",
    }

    # Seam A: send the WHOLE directive to mini-pool.
    ticket = mcp._pigeon_send_directive(
        sender_id="coder-1", project_id="proj-1", directive=directive
    )
    assert isinstance(ticket, int)

    # The Rust executor's INGEST is `client.poll("mini-pool")` — assert the same call
    # returns OUR ticket + the verbatim directive payload (receiver_id matched).
    r = httpx.get(f"{base}/pigeon/poll", params={"agent_id": "mini-pool"}, headers=headers)
    assert r.status_code == 200, r.text
    body = r.json()
    assert body["ticket_no"] == ticket
    assert body["payload"] == directive  # WHOLE directive carried, byte-for-byte

    # A directive sent to a DIFFERENT receiver must NOT show up on mini-pool's poll.
    r2 = httpx.get(f"{base}/pigeon/poll", params={"agent_id": "mini-pool"}, headers=headers)
    assert r2.json()["ticket_no"] is None  # queue drained — nothing else was mis-routed


def test_status_wait_returns_executor_outcome_on_done(dispatcher):
    base, headers = dispatcher
    directive = {"id": "dir-done", "status": "pending", "resultPath": "dir-done.json"}
    ticket = mcp._pigeon_send_directive(
        sender_id="coder-1", project_id="proj-1", directive=directive
    )

    # Simulate the Rust executor: claim (/poll) then post the terminal outcome (/done).
    claim = httpx.get(
        f"{base}/pigeon/poll", params={"agent_id": "mini-pool"}, headers=headers
    ).json()
    assert claim["ticket_no"] == ticket
    outcome = {"status": "done", "filesTouched": ["src/a.py"], "summary": "done"}
    done = httpx.post(
        f"{base}/pigeon/done",
        headers=headers,
        json={"ticket_no": ticket, "agent_id": "mini-pool", "result": outcome},
    )
    assert done.status_code == 200, done.text

    # Seam D: the wait helper returns {directiveId, result} with the executor's outcome.
    deadline = time.monotonic() + 10.0
    res = mcp._await_mini_directive_pigeon("dir-done", ticket, deadline)
    assert res["directiveId"] == "dir-done"
    assert res["result"] == outcome  # the /done payload IS the terminal MiniCoderOutcome


def test_status_wait_synthesizes_failed_on_failed_task(dispatcher):
    base, headers = dispatcher
    directive = {"id": "dir-fail", "status": "pending", "resultPath": "dir-fail.json"}
    ticket = mcp._pigeon_send_directive(
        sender_id="coder-1", project_id="proj-1", directive=directive
    )
    # Claim then FAIL it max_attempts times so it dead-letters to status 'failed'.
    for _ in range(5):
        claim = httpx.get(
            f"{base}/pigeon/poll", params={"agent_id": "mini-pool"}, headers=headers
        ).json()
        if claim["ticket_no"] is None:
            break
        httpx.post(
            f"{base}/pigeon/fail",
            headers=headers,
            json={"ticket_no": ticket, "agent_id": "mini-pool", "error": "boom"},
        )
        st = httpx.get(f"{base}/pigeon/status/{ticket}", headers=headers).json()
        if st["status"] == "failed":
            break

    st = httpx.get(f"{base}/pigeon/status/{ticket}", headers=headers).json()
    assert st["status"] == "failed", st

    deadline = time.monotonic() + 10.0
    res = mcp._await_mini_directive_pigeon("dir-fail", ticket, deadline)
    assert res["directiveId"] == "dir-fail"
    assert res["result"]["status"] == "failed"


def test_status_wait_times_out_to_failed_when_never_claimed(dispatcher):
    # A pending-but-never-claimed task at the deadline => executor-never-started `failed`.
    directive = {"id": "dir-stuck", "status": "pending"}
    ticket = mcp._pigeon_send_directive(
        sender_id="coder-1", project_id="proj-1", directive=directive
    )
    deadline = time.monotonic() + 0.2  # tiny cap — nothing claims it
    res = mcp._await_mini_directive_pigeon("dir-stuck", ticket, deadline)
    assert res["result"]["status"] == "failed"
    assert "did not start" in res["result"]["error"]
