"""P1 — the core mailbox: POST /pigeon/send, GET /pigeon/poll, POST /pigeon/done,
POST /pigeon/agent. delivery_mode, atomic claim (no double-delivery), and the
auto-reply with priority:10.

The decoupling proof test (test_temporal_decoupling_roundtrip) is the DEFINITION OF
DONE for the slice. Residency is the agents.status field (loaded|unloaded), flipped
via POST /pigeon/agent — no real model load/unload needed.

Run: oracle-data/venv/bin/python -m pytest pigeon -c pigeon/pytest.ini
"""
import asyncio
import contextlib

import httpx

from pigeon.dispatcher import build_app


@contextlib.asynccontextmanager
async def client_for(tmp_path, auth_token=None):
    app = build_app(db_path=str(tmp_path / "mb.sqlite"), auth_token=auth_token)
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://pigeon") as c:
        try:
            yield c, app
        finally:
            db = getattr(app.state, "db", None)
            if db is not None:
                await db.close()


async def _agent(c, agent_id, status, agent_type="local"):
    r = await c.post(
        "/pigeon/agent",
        json={"agent_id": agent_id, "agent_type": agent_type, "status": status},
    )
    assert r.status_code == 200, r.text


async def _send(c, sender, receiver, priority, payload, project="p"):
    r = await c.post(
        "/pigeon/send",
        json={
            "sender_id": sender,
            "receiver_id": receiver,
            "project_id": project,
            "priority": priority,
            "payload": payload,
        },
    )
    assert r.status_code == 200, r.text
    return r.json()


# ----------------------------------------------------------------- delivery_mode

async def test_send_to_unloaded_is_queued(tmp_path):
    async with client_for(tmp_path) as (c, _):
        await _agent(c, "R", "unloaded")
        body = await _send(c, "S", "R", 40, {"type": "edit_file", "instruction": "x"})
        assert body["delivery_mode"] == "queued"
        assert body["receiver_status"] == "unloaded"
        assert isinstance(body["ticket_no"], int)
        assert body["status"] == "pending"


async def test_send_to_loaded_is_immediate(tmp_path):
    async with client_for(tmp_path) as (c, _):
        await _agent(c, "R", "loaded")
        body = await _send(c, "S", "R", 40, {"k": 1})
        assert body["delivery_mode"] == "immediate"
        assert body["receiver_status"] == "loaded"


async def test_send_to_unknown_receiver_is_queued(tmp_path):
    async with client_for(tmp_path) as (c, _):
        body = await _send(c, "S", "ghost", 50, {})
        assert body["delivery_mode"] == "queued"
        assert body["receiver_status"] == "unloaded"


# ------------------------------------------------------------------------- poll

async def test_poll_empty_returns_null(tmp_path):
    async with client_for(tmp_path) as (c, _):
        r = await c.get("/pigeon/poll", params={"agent_id": "nobody"})
        assert r.status_code == 200
        assert r.json()["ticket_no"] is None
        assert r.json()["payload"] is None


async def test_poll_priority_order(tmp_path):
    async with client_for(tmp_path) as (c, _):
        await _send(c, "S", "R", 60, {"n": "low"})
        await _send(c, "S", "R", 10, {"n": "high"})
        r1 = (await c.get("/pigeon/poll", params={"agent_id": "R"})).json()
        r2 = (await c.get("/pigeon/poll", params={"agent_id": "R"})).json()
        assert r1["payload"]["n"] == "high"   # lower priority number = more urgent
        assert r2["payload"]["n"] == "low"


async def test_poll_does_not_redeliver_claimed(tmp_path):
    async with client_for(tmp_path) as (c, _):
        await _send(c, "S", "R", 50, {"only": 1})
        first = (await c.get("/pigeon/poll", params={"agent_id": "R"})).json()
        second = (await c.get("/pigeon/poll", params={"agent_id": "R"})).json()
        assert first["ticket_no"] is not None
        assert second["ticket_no"] is None


async def test_claim_race_single_winner(tmp_path):
    async with client_for(tmp_path) as (c, _):
        await _send(c, "S", "R", 50, {"only": 1})
        r1, r2 = await asyncio.gather(
            c.get("/pigeon/poll", params={"agent_id": "R"}),
            c.get("/pigeon/poll", params={"agent_id": "R"}),
        )
        tickets = [r1.json()["ticket_no"], r2.json()["ticket_no"]]
        non_null = [t for t in tickets if t is not None]
        assert len(non_null) == 1, tickets


# ----------------------------------------------------- THE decoupling proof test

async def test_temporal_decoupling_roundtrip(tmp_path):
    """Definition of done: a task survives the sender being unloaded, and the reply
    finds the sender when it comes back — proven without real model load/unload."""
    async with client_for(tmp_path) as (c, _):
        # R is NOT resident; S sends an urgent task (+ a less urgent one)
        await _agent(c, "R", "unloaded")
        send = await _send(c, "S", "R", 40, {"type": "edit_file", "instruction": "fix"})
        t1 = send["ticket_no"]
        assert send["delivery_mode"] == "queued"
        await _send(c, "S", "R", 70, {"type": "noop"})

        # S could now be unloaded — irrelevant, the task lives in SQLite.
        # R becomes resident and polls → gets the urgent task first.
        await _agent(c, "R", "loaded")
        poll = (await c.get("/pigeon/poll", params={"agent_id": "R"})).json()
        assert poll["ticket_no"] == t1
        assert poll["payload"]["instruction"] == "fix"

        # R completes → Pigeon auto-creates a reply to S with priority 10.
        # S is unloaded, so that reply is queued and waits.
        done = (await c.post(
            "/pigeon/done", json={"ticket_no": t1, "result": {"output": "done"}}
        )).json()
        t2 = done["reply_ticket_no"]
        assert t2 is not None

        # S returns to RAM and polls → receives its reply.
        await _agent(c, "S", "loaded")
        reply = (await c.get("/pigeon/poll", params={"agent_id": "S"})).json()
        assert reply["ticket_no"] == t2
        assert reply["payload"]["type"] == "task_result"
        assert reply["payload"]["original_ticket"] == t1
        assert reply["payload"]["result"] == {"output": "done"}


# ------------------------------------------------------ review fixes (blockers)

async def test_done_is_idempotent_no_duplicate_reply(tmp_path):
    # A second /done on the same ticket must NOT create a second reply (exactly-once).
    async with client_for(tmp_path) as (c, _):
        await _send(c, "S", "R", 50, {"x": 1})
        t1 = (await c.get("/pigeon/poll", params={"agent_id": "R"})).json()["ticket_no"]
        d1 = await c.post("/pigeon/done", json={"ticket_no": t1, "result": {"o": 1}})
        assert d1.status_code == 200
        d2 = await c.post("/pigeon/done", json={"ticket_no": t1, "result": {"o": 2}})
        assert d2.status_code == 409
        # S must have exactly ONE reply waiting
        await _agent(c, "S", "loaded")
        first = (await c.get("/pigeon/poll", params={"agent_id": "S"})).json()
        second = (await c.get("/pigeon/poll", params={"agent_id": "S"})).json()
        assert first["ticket_no"] is not None
        assert second["ticket_no"] is None


async def test_done_on_unpolled_task_rejected(tmp_path):
    # A task that was never polled (still 'pending') cannot be completed.
    async with client_for(tmp_path) as (c, _):
        t = (await _send(c, "S", "R", 50, {"x": 1}))["ticket_no"]
        r = await c.post("/pigeon/done", json={"ticket_no": t, "result": {}})
        assert r.status_code == 409


async def test_done_missing_ticket_404(tmp_path):
    async with client_for(tmp_path) as (c, _):
        r = await c.post("/pigeon/done", json={"ticket_no": 999, "result": {}})
        assert r.status_code == 404


async def test_auth_required_when_token_set(tmp_path):
    async with client_for(tmp_path, auth_token="tok") as (c, _):
        no_header = await c.post(
            "/pigeon/agent", json={"agent_id": "X", "agent_type": "local", "status": "loaded"}
        )
        assert no_header.status_code == 401
        ok = await c.post(
            "/pigeon/agent",
            json={"agent_id": "X", "agent_type": "local", "status": "loaded"},
            headers={"x-pigeon-auth-token": "tok"},
        )
        assert ok.status_code == 200
        assert (await c.get("/health")).status_code == 200  # health is exempt


async def test_auth_empty_token_is_disabled(tmp_path):
    async with client_for(tmp_path, auth_token="") as (c, _):
        r = await c.post(
            "/pigeon/agent", json={"agent_id": "X", "agent_type": "local", "status": "loaded"}
        )
        assert r.status_code == 200  # empty token == auth disabled
