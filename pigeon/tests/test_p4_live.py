"""P4 — live HTTP integration. Boot the REAL uvicorn service as a subprocess
(`python -m pigeon.dispatcher`) on a real loopback port and drive the full temporal
decoupling lifecycle over a real socket — proving it works as an actual service, not
just via in-process ASGITransport. This mirrors how the Rust supervisor will run it.
"""
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

import httpx
import pytest

REPO = Path(__file__).resolve().parents[2]   # .../Aspis-management
PYTHON = sys.executable


def _free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


@pytest.fixture
def live_server(tmp_path):
    port = _free_port()
    env = dict(os.environ)
    env["PYTHONPATH"] = str(REPO)
    env["PIGEON_PORT"] = str(port)
    env["PIGEON_SQLITE_PATH"] = str(tmp_path / "mb.sqlite")
    proc = subprocess.Popen(
        [PYTHON, "-m", "pigeon.dispatcher"],
        cwd=str(REPO),
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    base = f"http://127.0.0.1:{port}"
    try:
        deadline = time.time() + 15
        while time.time() < deadline:
            if proc.poll() is not None:
                out = proc.stdout.read().decode(errors="replace")
                raise RuntimeError(f"server exited early (rc={proc.returncode}):\n{out}")
            try:
                if httpx.get(base + "/health", timeout=0.5).status_code == 200:
                    break
            except httpx.TransportError:
                time.sleep(0.2)
        else:
            raise RuntimeError("server did not become ready in time")
        yield base
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


def test_live_health(live_server):
    r = httpx.get(live_server + "/health", timeout=5)
    assert r.status_code == 200
    body = r.json()
    assert body["service"] == "pigeon"
    assert body["auth"] == "disabled"


def test_live_decoupling_roundtrip(live_server):
    with httpx.Client(base_url=live_server, timeout=5) as c:
        # R not resident; S sends an urgent task
        c.post("/pigeon/agent", json={"agent_id": "R", "agent_type": "local", "status": "unloaded"})
        send = c.post(
            "/pigeon/send",
            json={"sender_id": "S", "receiver_id": "R", "project_id": "p",
                  "priority": 40, "payload": {"instruction": "fix"}},
        ).json()
        t1 = send["ticket_no"]
        assert send["delivery_mode"] == "queued"

        # R becomes resident, polls, completes
        c.post("/pigeon/agent", json={"agent_id": "R", "agent_type": "local", "status": "loaded"})
        poll = c.get("/pigeon/poll", params={"agent_id": "R"}).json()
        assert poll["ticket_no"] == t1
        assert poll["payload"]["instruction"] == "fix"

        done = c.post("/pigeon/done", json={"ticket_no": t1, "result": {"output": "ok"}}).json()
        t2 = done["reply_ticket_no"]
        assert t2 is not None

        # S returns and receives its reply
        c.post("/pigeon/agent", json={"agent_id": "S", "agent_type": "local", "status": "loaded"})
        reply = c.get("/pigeon/poll", params={"agent_id": "S"}).json()
        assert reply["ticket_no"] == t2
        assert reply["payload"]["type"] == "task_result"
        assert reply["payload"]["original_ticket"] == t1
        assert reply["payload"]["result"] == {"output": "ok"}
